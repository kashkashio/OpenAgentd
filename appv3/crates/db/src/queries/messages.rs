//! `session_messages` — ports `chat_service.py`, `chat_service_revert.py`
//! and `chat_service_queue.py`. Ordering is always `(seq, id)`.

use crate::codec::{db_id, json_db, new_id, now_db, parse_dt, py_isoformat};
use crate::models::{kind, ChatSession, SessionMessage, SEQ_STEP};
use crate::pool::DbPool;
use crate::queries::sessions::{bump_history_revision, get_session};
use anyhow::Result;
use serde_json::{Map, Value};

/// v2 history page size (`_HISTORY_PAGE_SIZE`).
pub const HISTORY_PAGE_SIZE: i64 = 100;

/// A message to persist (v2 `save_message` arguments).
#[derive(Debug, Default, Clone)]
pub struct NewMessage {
    pub role: String,
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Value>,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
    pub extra: Option<Map<String, Value>>,
    /// Explicit kind; derived like v2 when `None`.
    pub kind: Option<String>,
    pub is_summary: bool,
    pub pinned: Option<bool>,
    pub seq: Option<i64>,
    pub created_at: Option<String>,
}

impl NewMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: Some(content.into()), ..Default::default() }
    }
    pub fn assistant(content: Option<String>) -> Self {
        Self { role: "assistant".into(), content, ..Default::default() }
    }
    pub fn tool(call_id: impl Into<String>, name: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: "tool".into(), content: Some(content.into()), tool_call_id: Some(call_id.into()), name: Some(name.into()), ..Default::default() }
    }
}

pub async fn next_seq(pool: &DbPool, session_id: &str) -> Result<i64> {
    let max: Option<i64> = sqlx::query_scalar("SELECT MAX(seq) FROM session_messages WHERE session_id = ?").bind(db_id(session_id)).fetch_one(pool).await?;
    Ok(max.unwrap_or(0) + SEQ_STEP)
}

/// Midpoint strictly between two positions; ties when the gap is exhausted.
pub fn seq_between(prev: i64, next: i64) -> i64 {
    let gap = next - prev;
    if gap >= 2 {
        prev + gap / 2
    } else {
        prev
    }
}

pub async fn get_message(pool: &DbPool, id: &str) -> Result<Option<SessionMessage>> {
    Ok(sqlx::query_as::<_, SessionMessage>("SELECT * FROM session_messages WHERE id = ?").bind(db_id(id)).fetch_optional(pool).await?)
}

/// v2 `save_message`: derives `kind`/`pinned`, allocates `seq`, bumps the
/// structural revision for summaries.
pub async fn save_message(pool: &DbPool, session_id: &str, msg: NewMessage) -> Result<SessionMessage> {
    let sid = db_id(session_id);
    let extra_hidden = msg.extra.as_ref().and_then(|e| e.get("hidden_from_user")).map(|v| !v.is_null() && v != &Value::Bool(false)).unwrap_or(false);
    let row_kind = msg.kind.clone().unwrap_or_else(|| {
        if msg.is_summary {
            kind::SUMMARY.into()
        } else if extra_hidden {
            kind::NOTE.into()
        } else {
            kind::CHAT.into()
        }
    });
    let pinned = msg
        .pinned
        .unwrap_or_else(|| row_kind == kind::NOTE && msg.extra.as_ref().and_then(|e| e.get("hidden_from_summary")).map(|v| v.as_bool().unwrap_or(!v.is_null())).unwrap_or(false));
    let id = new_id();
    let extra = msg.extra.filter(|e| !e.is_empty()).map(Value::Object);
    let tool_calls = msg.tool_calls.filter(|t| !t.is_null());
    let created = msg.created_at.as_deref().and_then(crate::codec::db_dt).unwrap_or_else(now_db);
    // `seq` is allocated inside the INSERT: SQLite takes the write lock for
    // the whole statement, so concurrent saves (a queued message while the
    // agent writes its reply) cannot read the same MAX(seq).
    let row = sqlx::query_as::<_, SessionMessage>(
        r#"INSERT INTO session_messages
           (id, session_id, role, content, reasoning_content, tool_calls,
            tool_call_id, name, extra, created_at, seq, kind, pinned)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                   COALESCE(?, (SELECT COALESCE(MAX(seq), 0) + ? FROM session_messages WHERE session_id = ?)),
                   ?, ?)
           RETURNING *"#,
    )
    .bind(&id)
    .bind(&sid)
    .bind(&msg.role)
    .bind(&msg.content)
    .bind(&msg.reasoning_content)
    .bind(json_db(tool_calls.as_ref()))
    .bind(&msg.tool_call_id)
    .bind(&msg.name)
    .bind(json_db(extra.as_ref()))
    .bind(&created)
    .bind(msg.seq)
    .bind(SEQ_STEP)
    .bind(&sid)
    .bind(&row_kind)
    .bind(pinned)
    .fetch_one(pool)
    .await?;
    if row_kind == kind::SUMMARY {
        bump_history_revision(pool, &sid, true).await?;
    }
    Ok(row)
}

/// Update a row's content/extra in place (placeholder rewrites, usage).
pub async fn update_message_content(pool: &DbPool, id: &str, content: Option<&str>, extra: Option<&Map<String, Value>>) -> Result<()> {
    let extra_val = extra.filter(|e| !e.is_empty()).map(|e| Value::Object(e.clone()));
    sqlx::query("UPDATE session_messages SET content = ?, extra = ? WHERE id = ?").bind(content).bind(json_db(extra_val.as_ref())).bind(db_id(id)).execute(pool).await?;
    Ok(())
}

// ── Revert boundary / active summary ─────────────────────────────────────────

/// The staged undo boundary row, if any (v2 `revert_boundary`).
pub async fn revert_boundary(pool: &DbPool, session: &ChatSession) -> Result<Option<SessionMessage>> {
    let Some(mid) = session.revert_message_id() else { return Ok(None) };
    let row = get_message(pool, &mid.to_string()).await?;
    Ok(row.filter(|r| r.session_id == session.id))
}

/// Newest-created summary, optionally only those positioned before `boundary`.
pub async fn get_active_summary(pool: &DbPool, session_id: &str, boundary: Option<&SessionMessage>) -> Result<Option<SessionMessage>> {
    let sid = db_id(session_id);
    let row = match boundary {
        Some(b) => {
            sqlx::query_as::<_, SessionMessage>(
                "SELECT * FROM session_messages WHERE session_id = ? AND kind = 'summary' \
             AND (seq, id) < (?, ?) ORDER BY id DESC LIMIT 1",
            )
            .bind(&sid)
            .bind(b.seq)
            .bind(&b.id)
            .fetch_optional(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, SessionMessage>(
                "SELECT * FROM session_messages WHERE session_id = ? AND kind = 'summary' \
             ORDER BY id DESC LIMIT 1",
            )
            .bind(&sid)
            .fetch_optional(pool)
            .await?
        }
    };
    Ok(row)
}

/// Derived LLM window (v2 `_llm_window_rows`): pinned rows + active summary +
/// chat/note rows at/after it, before the undo boundary, `(seq, id)` order.
pub async fn llm_window_rows(pool: &DbPool, session_id: &str, exclude_queued: bool) -> Result<Vec<SessionMessage>> {
    let sid = db_id(session_id);
    // v2 tolerates a missing session row (no revert boundary then).
    let boundary = match get_session(pool, &sid).await? {
        Some(session) => revert_boundary(pool, &session).await?,
        None => None,
    };
    let summary = get_active_summary(pool, &sid, boundary.as_ref()).await?;

    let mut sql = String::from("SELECT * FROM session_messages WHERE session_id = ? AND kind != 'reverted'");
    if exclude_queued {
        sql.push_str(" AND kind != 'queued'");
    }
    if summary.is_some() {
        sql.push_str(" AND (pinned = 1 OR (seq, id) >= (?, ?)) AND (kind != 'summary' OR id = ?)");
    } else {
        sql.push_str(" AND kind != 'summary'");
    }
    if boundary.is_some() {
        sql.push_str(" AND (seq, id) < (?, ?)");
    }
    sql.push_str(" ORDER BY seq ASC, id ASC");
    let mut q = sqlx::query_as::<_, SessionMessage>(&sql).bind(&sid);
    if let Some(s) = &summary {
        q = q.bind(s.seq).bind(&s.id).bind(&s.id);
    }
    if let Some(b) = &boundary {
        q = q.bind(b.seq).bind(&b.id);
    }
    Ok(q.fetch_all(pool).await?)
}

// ── History (user-visible transcript) ────────────────────────────────────────

const USER_VISIBLE: &str = "kind NOT IN ('note', 'reverted')";

/// Newest-first page, returned chronological. `(rows, has_more, boundary)`.
pub async fn history_page(pool: &DbPool, session_id: &str, before: Option<(i64, Option<String>)>) -> Result<(Vec<SessionMessage>, bool, Option<SessionMessage>)> {
    let sid = db_id(session_id);
    let mut sql = format!("SELECT * FROM session_messages WHERE session_id = ? AND {USER_VISIBLE}");
    match &before {
        Some((_, Some(_))) => sql.push_str(" AND (seq, id) < (?, ?)"),
        Some((_, None)) => sql.push_str(" AND seq < ?"),
        None => {}
    }
    sql.push_str(" ORDER BY seq DESC, id DESC LIMIT ?");
    let mut q = sqlx::query_as::<_, SessionMessage>(&sql).bind(&sid);
    if let Some((seq, id)) = &before {
        q = q.bind(*seq);
        if let Some(id) = id {
            q = q.bind(db_id(id));
        }
    }
    let mut rows = q.bind(HISTORY_PAGE_SIZE + 1).fetch_all(pool).await?;
    let has_more = rows.len() as i64 > HISTORY_PAGE_SIZE;
    rows.truncate(HISTORY_PAGE_SIZE as usize);
    rows.reverse();
    let boundary = if has_more { rows.first().cloned() } else { None };
    Ok((rows, has_more, boundary))
}

/// Rows created after the uuid7 `since` cursor, `(seq, id)` ordered.
/// Returns `(rows, truncated)`.
pub async fn history_since(pool: &DbPool, session_id: &str, since_id: &str, limit: i64) -> Result<(Vec<SessionMessage>, bool)> {
    let sql = format!(
        "SELECT * FROM session_messages WHERE session_id = ? AND id > ? AND {USER_VISIBLE} \
         ORDER BY id ASC LIMIT ?"
    );
    let mut rows = sqlx::query_as::<_, SessionMessage>(&sql).bind(db_id(session_id)).bind(db_id(since_id)).bind(limit + 1).fetch_all(pool).await?;
    let truncated = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    rows.sort_by(|a, b| (a.seq, &a.id).cmp(&(b.seq, &b.id)));
    Ok((rows, truncated))
}

/// Translate a legacy `(created_at, id)` history cursor into `(seq, id)`.
pub async fn resolve_legacy_history_cursor(pool: &DbPool, session_id: &str, before: &str, before_id: Option<&str>) -> Result<Option<(i64, String)>> {
    let sid = db_id(session_id);
    if let Some(bid) = before_id {
        if let Some(row) = get_message(pool, bid).await? {
            if row.session_id == sid {
                return Ok(Some((row.seq, row.id)));
            }
        }
    }
    let Some(dt) = parse_dt(before) else { return Ok(None) };
    Ok(sqlx::query_as::<_, (i64, String)>(
        "SELECT seq, id FROM session_messages WHERE session_id = ? AND created_at >= ? \
         ORDER BY seq ASC, id ASC LIMIT 1",
    )
    .bind(&sid)
    .bind(crate::codec::dt_db(&dt))
    .fetch_optional(pool)
    .await?)
}

/// Legacy timestamp delta watermark → uuid7 cursor (v2 `resolve_legacy_delta_cursor`).
pub async fn resolve_legacy_delta_cursor(pool: &DbPool, root_id: &str, since: &str) -> Result<String> {
    let Some(dt) = parse_dt(since) else { return Ok("0".repeat(32)) };
    let row: Option<String> = sqlx::query_scalar(
        "SELECT id FROM session_messages WHERE session_id IN \
         (SELECT id FROM chat_sessions WHERE id = ? OR parent_session_id = ?) \
         AND created_at <= ? ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(db_id(root_id))
    .bind(db_id(root_id))
    .bind(crate::codec::dt_db(&dt))
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or_else(|| "0".repeat(32)))
}

/// `(estimated_cost_usd, completion_tokens)` over user-visible rows.
pub async fn session_usage_totals(pool: &DbPool, session_id: &str) -> Result<(f64, i64)> {
    let sql = format!(
        "SELECT CAST(COALESCE(SUM(json_extract(extra, '$.usage.cost.estimated_usd')), 0) AS REAL), \
                CAST(COALESCE(SUM(json_extract(extra, '$.usage.output')), 0) AS REAL) \
         FROM session_messages WHERE session_id = ? AND {USER_VISIBLE}"
    );
    let (cost, out): (f64, f64) = sqlx::query_as(&sql).bind(db_id(session_id)).fetch_one(pool).await?;
    Ok((appv3_core::pymath::py_round(cost, 8), out as i64))
}

/// [`session_usage_totals`] for several sessions in one scan, keyed by the
/// caller's id. A session with no rows maps to `(0.0, 0)`.
pub async fn session_usage_totals_many(pool: &DbPool, session_ids: &[&str]) -> Result<std::collections::HashMap<String, (f64, i64)>> {
    let mut out: std::collections::HashMap<String, (f64, i64)> = session_ids.iter().map(|id| (id.to_string(), (0.0, 0))).collect();
    if session_ids.is_empty() {
        return Ok(out);
    }
    let by_db: std::collections::HashMap<String, &str> = session_ids.iter().map(|id| (db_id(id), *id)).collect();
    let marks = vec!["?"; by_db.len()].join(", ");
    let sql = format!(
        "SELECT session_id, \
                CAST(COALESCE(SUM(json_extract(extra, '$.usage.cost.estimated_usd')), 0) AS REAL), \
                CAST(COALESCE(SUM(json_extract(extra, '$.usage.output')), 0) AS REAL) \
         FROM session_messages WHERE session_id IN ({marks}) AND {USER_VISIBLE} GROUP BY session_id"
    );
    let mut q = sqlx::query_as::<_, (String, f64, f64)>(&sql);
    for id in by_db.keys() {
        q = q.bind(id);
    }
    for (sid, cost, completion) in q.fetch_all(pool).await? {
        if let Some(caller) = by_db.get(&sid) {
            out.insert(caller.to_string(), (appv3_core::pymath::py_round(cost, 8), completion as i64));
        }
    }
    Ok(out)
}

/// Newest row cursor `(seq, id)` of a session.
pub async fn get_history_cursor(pool: &DbPool, session_id: &str) -> Result<Option<(i64, String)>> {
    Ok(sqlx::query_as::<_, (i64, String)>("SELECT seq, id FROM session_messages WHERE session_id = ? ORDER BY seq DESC, id DESC LIMIT 1")
        .bind(db_id(session_id))
        .fetch_optional(pool)
        .await?)
}

// ── Undo / redo ──────────────────────────────────────────────────────────────

/// Real human-authored user rows (not another agent's inbox message).
const REAL_USER: &str = "role = 'user' AND (json_extract(extra, '$.from_agent') IS NULL \
                         OR json_extract(extra, '$.from_agent') = 'user')";

/// Next undo target (v2 `undo_session_messages` target selection).
pub async fn find_undo_target(pool: &DbPool, session: &ChatSession) -> Result<Option<SessionMessage>> {
    let boundary = revert_boundary(pool, session).await?;
    let active = get_active_summary(pool, &session.id, boundary.as_ref()).await?;
    let mut sql = format!("SELECT * FROM session_messages WHERE session_id = ? AND {REAL_USER} AND kind IN ('chat', 'summary')");
    if active.is_some() {
        sql.push_str(" AND (kind = 'summary' OR (seq, id) >= (?, ?))");
    }
    if boundary.is_some() {
        sql.push_str(" AND (seq, id) < (?, ?)");
    }
    sql.push_str(" ORDER BY seq DESC, id DESC LIMIT 1");
    let mut q = sqlx::query_as::<_, SessionMessage>(&sql).bind(&session.id);
    if let Some(a) = &active {
        q = q.bind(a.seq).bind(&a.id);
    }
    if let Some(b) = &boundary {
        q = q.bind(b.seq).bind(&b.id);
    }
    let row = q.fetch_optional(pool).await?;
    Ok(row.filter(|r| (r.kind == kind::CHAT || r.kind == kind::SUMMARY) && r.is_from_user()))
}

/// Next real user chat row after the boundary (v2 redo target).
pub async fn find_redo_target(pool: &DbPool, session: &ChatSession, boundary: &SessionMessage) -> Result<Option<SessionMessage>> {
    let sql = format!(
        "SELECT * FROM session_messages WHERE session_id = ? AND {REAL_USER} AND kind = 'chat' \
         AND (seq, id) > (?, ?) ORDER BY seq ASC, id ASC LIMIT 1"
    );
    Ok(sqlx::query_as::<_, SessionMessage>(&sql).bind(&session.id).bind(boundary.seq).bind(&boundary.id).fetch_optional(pool).await?)
}

/// The `chat_sessions.revert` blob v2 writes for a boundary at `target`.
pub fn revert_state(target: &SessionMessage, anchor: Option<&str>) -> Value {
    let created = parse_dt(&target.created_at).map(|d| py_isoformat(&d)).unwrap_or_else(|| target.created_at.clone());
    let mut m = Map::new();
    m.insert("message_id".into(), Value::String(crate::codec::api_uuid(&target.id)));
    m.insert("created_at".into(), Value::String(created));
    if let Some(a) = anchor.filter(|a| !a.is_empty()) {
        m.insert("snapshot".into(), Value::String(a.to_string()));
    }
    Value::Object(m)
}

/// Materialise an undo: rows at/after the boundary become `reverted`
/// (queued rows survive). Clears the boundary. v2 `cleanup_reverted_tail`.
pub async fn cleanup_reverted_tail(pool: &DbPool, session_id: &str) -> Result<u64> {
    let sid = db_id(session_id);
    let Some(session) = get_session(pool, &sid).await? else { return Ok(0) };
    let Some(boundary) = revert_boundary(pool, &session).await? else { return Ok(0) };
    let mut tx = pool.begin().await?;
    let cleaned = sqlx::query(
        "UPDATE session_messages SET kind = 'reverted' WHERE session_id = ? \
         AND (seq, id) >= (?, ?) AND kind != 'queued'",
    )
    .bind(&sid)
    .bind(boundary.seq)
    .bind(&boundary.id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("UPDATE chat_sessions SET revert = 'null', updated_at = ? WHERE id = ?").bind(now_db()).bind(&sid).execute(&mut *tx).await?;
    tx.commit().await?;
    if cleaned > 0 {
        bump_history_revision(pool, &sid, true).await?;
    }
    Ok(cleaned)
}

/// v2 `exclude_messages_before_summary`: anchor a fresh summary so it covers
/// all but the last `keep_last_n` window rows. Returns rows covered.
pub async fn exclude_messages_before_summary(pool: &DbPool, session_id: &str, summary_id: &str, keep_last_n: i64) -> Result<i64> {
    let sid = db_id(session_id);
    let Some(summary) = get_message(pool, summary_id).await? else { return Ok(0) };
    if summary.session_id != sid {
        return Ok(0);
    }
    let previous = get_active_summary(pool, &sid, None).await?;
    let mut cond = String::from("session_id = ? AND kind IN ('chat', 'note') AND (seq, id) < (?, ?)");
    let restrict = previous.as_ref().filter(|p| p.id != summary.id).cloned();
    if restrict.is_some() {
        cond.push_str(" AND (pinned = 1 OR (seq, id) >= (?, ?))");
    }
    let count_sql = format!("SELECT COUNT(*) FROM session_messages WHERE {cond}");
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql).bind(sid.clone()).bind(summary.seq).bind(summary.id.clone());
    if let Some(p) = &restrict {
        count_q = count_q.bind(p.seq).bind(p.id.clone());
    }
    let total_before = count_q.fetch_one(pool).await?;

    let mut new_seq = summary.seq;
    if keep_last_n > 0 && total_before > 0 {
        let offset = keep_last_n.min(total_before) - 1;
        let kept_sql = format!("SELECT * FROM session_messages WHERE {cond} ORDER BY seq DESC, id DESC LIMIT 1 OFFSET {offset}");
        let mut kept_q = sqlx::query_as::<_, SessionMessage>(&kept_sql).bind(sid.clone()).bind(summary.seq).bind(summary.id.clone());
        if let Some(p) = &restrict {
            kept_q = kept_q.bind(p.seq).bind(p.id.clone());
        }
        let first_kept = kept_q.fetch_optional(pool).await?;
        if let Some(fk) = first_kept {
            let prev: Option<i64> = sqlx::query_scalar("SELECT MAX(seq) FROM session_messages WHERE session_id = ? AND seq < ?").bind(&sid).bind(fk.seq).fetch_one(pool).await?;
            new_seq = seq_between(prev.unwrap_or(0), fk.seq);
            sqlx::query("UPDATE session_messages SET seq = ? WHERE id = ?").bind(new_seq).bind(&summary.id).execute(pool).await?;
        }
    }
    let covered = if keep_last_n <= 0 { total_before } else { (total_before - keep_last_n).max(0) };
    sqlx::query(
        "UPDATE session_messages SET pinned = 0 WHERE session_id = ? AND pinned = 1 \
         AND (seq < ? OR (seq = ? AND id < ?))",
    )
    .bind(&sid)
    .bind(new_seq)
    .bind(new_seq)
    .bind(&summary.id)
    .execute(pool)
    .await?;
    if covered > 0 {
        bump_history_revision(pool, &sid, true).await?;
    }
    Ok(covered)
}

// ── Queue ────────────────────────────────────────────────────────────────────

/// v2 `save_queued_user_message`.
pub async fn save_queued_user_message(pool: &DbPool, session_id: &str, content: &str, extra: Option<Map<String, Value>>) -> Result<SessionMessage> {
    let mut e = extra.unwrap_or_default();
    e.insert("queue_status".into(), Value::String("queued".into()));
    e.insert("queued_at".into(), Value::String(py_isoformat(&chrono::Utc::now())));
    save_message(pool, session_id, NewMessage { kind: Some(kind::QUEUED.into()), extra: Some(e), ..NewMessage::user(content) }).await
}

/// True when the session has any queued user rows awaiting promotion.
pub async fn has_queued_user_messages(pool: &DbPool, session_id: &str) -> Result<bool> {
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session_messages WHERE session_id = ? AND role = 'user' AND kind = 'queued'").bind(db_id(session_id)).fetch_one(pool).await?;
    Ok(n > 0)
}

/// Promote all queued rows to `chat` at the tail (v2 `_promote_queued`).
/// `snapshot` is stored on each promoted row's `extra`.
pub async fn release_queued_user_messages(pool: &DbPool, session_id: &str, snapshot: Option<&str>) -> Result<Vec<SessionMessage>> {
    let sid = db_id(session_id);
    let queued = sqlx::query_as::<_, SessionMessage>(
        "SELECT * FROM session_messages WHERE session_id = ? AND role = 'user' AND kind = 'queued' \
         ORDER BY seq ASC, id ASC",
    )
    .bind(&sid)
    .fetch_all(pool)
    .await?;
    if queued.is_empty() {
        return Ok(queued);
    }
    let released = chrono::Utc::now();
    // IMMEDIATE takes the write lock up front, so the tail read below and
    // the updates cannot interleave with a concurrent `save_message`.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let max: Option<i64> = sqlx::query_scalar("SELECT MAX(seq) FROM session_messages WHERE session_id = ?").bind(&sid).fetch_one(&mut *tx).await?;
    let base = max.unwrap_or(0) + SEQ_STEP;
    for (i, row) in queued.iter().enumerate() {
        let mut extra = match row.extra_json() {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        extra.remove("queue_status");
        extra.remove("queued_at");
        if let Some(s) = snapshot {
            extra.insert("snapshot".into(), Value::String(s.into()));
        }
        let extra_v = if extra.is_empty() { None } else { Some(Value::Object(extra)) };
        let created = released + chrono::Duration::microseconds(i as i64);
        sqlx::query("UPDATE session_messages SET kind = 'chat', seq = ?, created_at = ?, extra = ? WHERE id = ?")
            .bind(base + i as i64 * SEQ_STEP)
            .bind(crate::codec::dt_db(&created))
            .bind(json_db(extra_v.as_ref()))
            .bind(&row.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    bump_history_revision(pool, &sid, true).await?;
    let mut out = Vec::with_capacity(queued.len());
    for row in &queued {
        if let Some(r) = get_message(pool, &row.id).await? {
            out.push(r);
        }
    }
    Ok(out)
}

/// v2 `cancel_queued_user_message`. Returns false when not a queued row of
/// this session. Attachment files listed in `extra.attachments[].path` are
/// deleted best-effort, as are mention rows tied to it.
pub async fn cancel_queued_user_message(pool: &DbPool, session_id: &str, message_id: &str) -> Result<bool> {
    let sid = db_id(session_id);
    let Some(row) = get_message(pool, message_id).await? else { return Ok(false) };
    if row.session_id != sid || row.kind != kind::QUEUED {
        return Ok(false);
    }
    if let Some(Value::Array(atts)) = row.extra_json().and_then(|e| e.get("attachments").cloned()) {
        for att in atts {
            if let Some(p) = att.get("path").and_then(|p| p.as_str()) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM session_messages WHERE session_id = ? \
         AND json_extract(extra, '$.attachment_for_message_id') IN (?, ?)",
    )
    .bind(&sid)
    .bind(crate::codec::api_uuid(&row.id))
    .bind(&row.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM session_messages WHERE id = ?").bind(&row.id).execute(&mut *tx).await?;
    tx.commit().await?;
    bump_history_revision(pool, &sid, true).await?;
    Ok(true)
}

/// Queued rows of a session (for the UI queue display / activation).
pub async fn list_queued_messages(pool: &DbPool, session_id: &str) -> Result<Vec<SessionMessage>> {
    Ok(sqlx::query_as::<_, SessionMessage>("SELECT * FROM session_messages WHERE session_id = ? AND kind = 'queued' ORDER BY seq ASC, id ASC")
        .bind(db_id(session_id))
        .fetch_all(pool)
        .await?)
}

/// v2 `_mark_last_assistant_interrupted`: newest assistant row by
/// `created_at` gets `extra.interrupted = true`.
pub async fn mark_last_assistant_interrupted(pool: &DbPool, session_id: &str) -> Result<()> {
    let row = sqlx::query_as::<_, SessionMessage>("SELECT * FROM session_messages WHERE session_id = ? AND role = 'assistant' ORDER BY created_at DESC LIMIT 1")
        .bind(db_id(session_id))
        .fetch_optional(pool)
        .await?;
    if let Some(row) = row {
        let mut extra = match row.extra_json() {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
        extra.insert("interrupted".into(), Value::Bool(true));
        sqlx::query("UPDATE session_messages SET extra = ? WHERE id = ?").bind(json_db(Some(&Value::Object(extra)))).bind(&row.id).execute(pool).await?;
    }
    Ok(())
}

/// Content of the newest assistant row by `(seq, id)` (subagent deliverables).
pub async fn last_assistant_content(pool: &DbPool, session_id: &str) -> Result<Option<String>> {
    let row: Option<(Option<String>,)> = sqlx::query_as("SELECT content FROM session_messages WHERE session_id = ? AND role = 'assistant' ORDER BY seq DESC, id DESC LIMIT 1")
        .bind(db_id(session_id))
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.0))
}

/// Direct `SessionMessage(...)` insert with model defaults (seq 0, kind chat),
/// as v2 `send_subagent_message` does for the `ask_lead` answer.
pub async fn insert_raw_tool_message(pool: &DbPool, session_id: &str, content: &str, tool_call_id: &str, name: &str) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO session_messages
           (id, session_id, role, content, reasoning_content, tool_calls,
            tool_call_id, name, extra, created_at, seq, kind, pinned)
           VALUES (?, ?, 'tool', ?, NULL, 'null', ?, ?, 'null', ?, 0, 'chat', 0)"#,
    )
    .bind(new_id())
    .bind(db_id(session_id))
    .bind(content)
    .bind(tool_call_id)
    .bind(name)
    .bind(now_db())
    .execute(pool)
    .await?;
    Ok(())
}

/// First `role='tool'` row for *tool_call_id* in a session (`_load_bound_mcp_app`).
pub async fn find_tool_message(pool: &DbPool, session_id: &str, tool_call_id: &str) -> Result<Option<SessionMessage>> {
    Ok(sqlx::query_as::<_, SessionMessage>("SELECT * FROM session_messages WHERE session_id = ? AND role = 'tool' AND tool_call_id = ? LIMIT 1")
        .bind(db_id(session_id))
        .bind(tool_call_id)
        .fetch_optional(pool)
        .await?)
}
