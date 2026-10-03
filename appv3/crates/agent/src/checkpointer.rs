//! `SQLiteCheckpointer` — port of `app/agent/checkpointer.py`.
//!
//! v2 tracks persisted messages by Python object identity; here a message is
//! persisted exactly when it carries a `db_id`.

use crate::history::to_new_message;
use crate::hooks::{AgentState, RunContext};
use crate::stream_store::store;
use anyhow::Result;
use appv3_db::{self as db, DbPool, SEQ_STEP};
use appv3_providers::ChatMessage;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct Checkpointer {
    pool: DbPool,
    stream_session_id: Option<String>,
    agent_name: Option<String>,
    flushed_pinned: Mutex<HashMap<String, bool>>,
    seeded_tokens: Mutex<i64>,
}

/// `_last_prompt_tokens_from_history`.
fn last_prompt_tokens(history: &[ChatMessage]) -> i64 {
    for m in history.iter().rev() {
        if m.meta().is_summary() {
            continue;
        }
        if let Some(Value::Object(u)) = m.meta().extra.as_ref().and_then(|e| e.get("usage")) {
            if let Some(n) = u.get("input").and_then(|v| v.as_i64()) {
                return n;
            }
        }
    }
    0
}

impl Checkpointer {
    pub fn new(pool: DbPool, stream_session_id: Option<String>, agent_name: Option<String>) -> Self {
        Self { pool, stream_session_id, agent_name, flushed_pinned: Mutex::new(HashMap::new()), seeded_tokens: Mutex::new(0) }
    }

    pub fn mark_loaded(&self, messages: &[ChatMessage]) {
        let mut p = self.flushed_pinned.lock().unwrap();
        for m in messages {
            if let Some(id) = &m.meta().db_id {
                p.entry(id.clone()).or_insert(m.meta().pinned);
            }
        }
        let t = last_prompt_tokens(messages);
        if t > 0 {
            *self.seeded_tokens.lock().unwrap() = t;
        }
    }

    pub fn seed_state(&self, state: &mut AgentState) {
        let t = *self.seeded_tokens.lock().unwrap();
        if t > 0 {
            state.usage.last_prompt_tokens = t;
        }
    }

    async fn seq_before(conn: &mut sqlx::SqliteConnection, anchor_id: &str) -> Result<Option<i64>> {
        let Some(row) = db::get_message(&mut *conn, anchor_id).await? else {
            return Ok(None);
        };
        let prev: Option<i64> =
            sqlx::query_scalar("SELECT MAX(seq) FROM session_messages WHERE session_id = ? AND seq < ?").bind(&row.session_id).bind(row.seq).fetch_one(&mut *conn).await?;
        Ok(Some(db::seq_between(prev.unwrap_or(0), row.seq)))
    }

    /// Persist new messages and flush `pinned` flips. Never fails the turn.
    pub async fn sync(&self, ctx: &RunContext, state: &mut AgentState) {
        if ctx.session_id.is_none() {
            return;
        }
        if let Err(e) = self.sync_inner(ctx, state).await {
            tracing::error!("checkpointer_sync_failed session_id={:?} error={}", ctx.session_id, e);
        }
    }

    async fn sync_inner(&self, ctx: &RunContext, state: &mut AgentState) -> Result<()> {
        let sid = ctx.session_id.clone().unwrap_or_default();
        let pin_updates: Vec<(String, bool)> = {
            let flushed = self.flushed_pinned.lock().unwrap();
            state
                .messages
                .iter()
                .filter(|m| !matches!(m, ChatMessage::System { .. }))
                .filter_map(|m| {
                    let id = m.meta().db_id.as_ref()?;
                    (flushed.get(id).copied().unwrap_or(false) != m.meta().pinned).then(|| (id.clone(), m.meta().pinned))
                })
                .collect()
        };
        let new_idx: Vec<usize> = (0..state.messages.len()).filter(|&i| state.messages[i].meta().db_id.is_none()).collect();
        if !new_idx.is_empty() || !pin_updates.is_empty() {
            // One write transaction per sync: the batch lands whole or not at
            // all (unsaved messages keep no `db_id` and go again next sync).
            // IMMEDIATE takes the write lock up front, so the reads below
            // cannot go stale under a concurrent `save_message`.
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            // Summary anchors (`_summary_anchor_ids`).
            let mut anchored: HashMap<usize, i64> = HashMap::new();
            for &i in &new_idx {
                if !state.messages[i].meta().is_summary() {
                    continue;
                }
                let anchor =
                    state.messages[i + 1..].iter().find(|m| m.meta().db_id.is_some() && !m.meta().exclude_from_context && !m.meta().pinned).and_then(|m| m.meta().db_id.clone());
                if let Some(a) = anchor {
                    if let Some(seq) = Self::seq_before(&mut tx, &a).await? {
                        anchored.insert(i, seq);
                    }
                }
            }
            for (flag, ids) in [
                (true, pin_updates.iter().filter(|(_, v)| *v).map(|(k, _)| k.clone()).collect::<Vec<_>>()),
                (false, pin_updates.iter().filter(|(_, v)| !*v).map(|(k, _)| k.clone()).collect()),
            ] {
                for id in ids {
                    sqlx::query("UPDATE session_messages SET pinned = ? WHERE id = ?").bind(flag).bind(db::codec::db_id(&id)).execute(&mut *tx).await?;
                }
            }
            let mut tail: Option<i64> = None;
            let mut saved: Vec<(usize, String)> = Vec::with_capacity(new_idx.len());
            for &i in &new_idx {
                let m = &state.messages[i];
                let save = match m {
                    ChatMessage::Assistant(a) => {
                        let has = a.content.as_deref().map(|c| !c.trim().is_empty()).unwrap_or(false)
                            || a.reasoning_content.as_deref().map(|c| !c.trim().is_empty()).unwrap_or(false)
                            || a.tool_calls.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
                            || a.reasoning_items.as_ref().map(|t| !t.is_empty()).unwrap_or(false)
                            || a.meta.is_summary();
                        if !has {
                            tracing::debug!("checkpointer_skip_empty_assistant session_id={}", sid);
                        }
                        has
                    }
                    ChatMessage::Tool { .. } => true,
                    ChatMessage::User { meta, .. } => meta.is_summary() || meta.extra.as_ref().and_then(|e| e.get("hidden_from_user")).map(crate::util::truthy).unwrap_or(false),
                    ChatMessage::System { .. } => false,
                };
                if !save {
                    continue;
                }
                let seq = match (m, anchored.get(&i)) {
                    (ChatMessage::Tool { .. }, _) | (_, None) => {
                        let next = match tail {
                            None => db::next_seq(&mut *tx, &sid).await?,
                            Some(t) => t + SEQ_STEP,
                        };
                        tail = Some(next);
                        next
                    }
                    (_, Some(s)) => *s,
                };
                let mut nm = to_new_message(m);
                nm.is_summary = m.meta().is_summary();
                nm.pinned = Some(m.meta().pinned);
                nm.seq = Some(seq);
                let id = db::save_message_id(&mut tx, &sid, nm).await?;
                saved.push((i, db::codec::api_uuid(&id)));
            }
            if !pin_updates.is_empty() {
                db::bump_history_revision(&mut *tx, &sid, true).await?;
            }
            tx.commit().await?;
            let mut flushed = self.flushed_pinned.lock().unwrap();
            for (i, id) in saved {
                let pinned = state.messages[i].meta().pinned;
                state.messages[i].meta_mut().db_id = Some(id.clone());
                flushed.insert(id, pinned);
            }
            for (k, v) in pin_updates {
                flushed.insert(k, v);
            }
        }
        if let (Some(ss), Some(agent)) = (&self.stream_session_id, &self.agent_name) {
            store().commit_agent_content(ss, agent);
        }
        Ok(())
    }
}
