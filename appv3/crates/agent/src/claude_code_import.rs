//! Import Claude Code transcripts (`~/.claude/projects/<project>/<id>.jsonl`)
//! into OpenAgentd sessions.
//!
//! Each transcript becomes a session with Claude's own session id, so a
//! `claude-code:*` turn in OpenAgentd resumes the same conversation
//! (`claude --resume <id>`). Sub-agent transcripts under `<id>/subagents/`
//! (including workflow runs) become child sessions. Assistant and tool rows go
//! through the same [`StreamParser`] as live Claude Code turns, so imported
//! chats render exactly like ones run here. Nothing is trimmed: images and
//! documents keep their original blocks in `extra.claude_code_content`.
//!
//! Re-running is safe. A session OpenAgentd created itself (its live Claude
//! Code turns write transcripts too) is skipped; a session this importer
//! created is topped up with lines newer than its last imported row. The
//! transcript format is internal to Claude Code, so unknown line types are
//! skipped rather than failing the import.

use crate::claude_code::{Action, StreamParser};
use appv3_db::{self as db, DbPool, NewMessage};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

const TITLE_CHARS: usize = 100;
const AGENT: &str = "code";
/// Namespace for sub-agent session ids (stable across re-imports).
const SUBAGENT_NS: uuid::Uuid = uuid::Uuid::from_u128(0x6f61_6764_636c_6175_6465_636f_6465_7375);

/// The default transcript root, `~/.claude/projects`.
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude").join("projects"))
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub root: PathBuf,
    /// Only project directories whose name contains this.
    pub project: Option<String>,
    pub subagents: bool,
    /// Also workflow runs (`subagents/workflows/**`): often thousands of
    /// agents per session, which makes that session slow to open.
    pub workflows: bool,
    pub dry_run: bool,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SessionReport {
    pub id: String,
    pub title: String,
    pub workspace: String,
    /// `new`, `updated`, `unchanged`, `skipped` (OpenAgentd's own session) or `error`.
    pub status: String,
    pub messages: usize,
    pub subagents: usize,
    pub detail: Option<String>,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    pub sessions: Vec<SessionReport>,
    pub messages: usize,
    pub subagents: usize,
    pub workspaces: usize,
}

/// One transcript, converted.
#[derive(Debug, Default)]
pub struct Transcript {
    pub session_id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub agent_name: Option<String>,
    /// `claude-code:<model>` of the newest assistant message.
    pub model: Option<String>,
    pub first_at: Option<String>,
    pub last_at: Option<String>,
    pub rows: Vec<NewMessage>,
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|x| !x.is_empty())
}

fn usage_dict(u: &Value) -> Option<Value> {
    let n = |k: &str| u[k].as_i64().unwrap_or(0);
    if !u.is_object() {
        return None;
    }
    let mut out = json!({
        "input": n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens"),
        "output": n("output_tokens"),
    });
    if let Some(c) = u["cache_read_input_tokens"].as_i64() {
        out["cache"] = json!(c);
    }
    Some(out)
}

fn has_non_text(blocks: &[Value]) -> bool {
    blocks.iter().any(|b| !matches!(b["type"].as_str(), Some("text") | Some("tool_result")))
        || blocks.iter().filter(|b| b["type"] == "tool_result").any(|b| b["content"].as_array().is_some_and(|c| has_non_text(c)))
}

/// A user prompt's text, with placeholders for images and documents.
fn prompt_text(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => b["text"].as_str().map(str::to_string),
                Some("image") => Some("[image]".into()),
                Some("document") => Some(format!("[document: {}]", s(b, "title").or_else(|| s(&b["source"], "media_type")).unwrap_or("file"))),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn mark(msg: &mut NewMessage, created_at: Option<&str>) {
    let extra = msg.extra.get_or_insert_with(Map::new);
    extra.insert("claude_code".into(), json!(true));
    extra.insert("claude_code_import".into(), json!(true));
    if msg.created_at.is_none() {
        msg.created_at = created_at.map(str::to_string);
    }
}

/// The assistant message being assembled, for the row the parser flushes.
#[derive(Default)]
struct Current {
    id: String,
    usage: Option<Value>,
    model: Option<String>,
    at: Option<String>,
}

/// Convert transcript lines to rows. Pure: no I/O.
pub fn convert_lines(lines: &[Value]) -> Transcript {
    let mut t = Transcript::default();
    let mut parser = StreamParser::new(AGENT, "claude-code:default");
    let mut cur = Current::default();
    let mut custom_title = None;
    let mut ai_title = None;
    let mut first_prompt: Option<String> = None;

    let take = |actions: Vec<Action>, cur: &Current, tool_at: Option<&str>, raw_results: &Map<String, Value>, rows: &mut Vec<NewMessage>| {
        for a in actions {
            let Action::Save(mut m) = a else { continue };
            if m.role == "assistant" {
                let extra = m.extra.get_or_insert_with(Map::new);
                if let Some(model) = &cur.model {
                    extra.insert("model".into(), json!(model));
                }
                if let Some(u) = &cur.usage {
                    extra.insert("usage".into(), u.clone());
                }
                mark(&mut m, cur.at.as_deref());
            } else {
                if let Some(raw) = m.tool_call_id.as_deref().and_then(|id| raw_results.get(id)) {
                    m.extra.get_or_insert_with(Map::new).insert("claude_code_content".into(), raw.clone());
                }
                mark(&mut m, tool_at);
            }
            rows.push(m);
        }
    };

    for line in lines {
        match line["type"].as_str().unwrap_or("") {
            "custom-title" => custom_title = s(line, "customTitle").map(str::to_string),
            "ai-title" => ai_title = s(line, "aiTitle").map(str::to_string),
            "agent-name" => t.agent_name = s(line, "agentName").map(str::to_string),
            "assistant" => {
                let msg = &line["message"];
                let id = s(msg, "id").unwrap_or("").to_string();
                let actions = parser.handle(line);
                take(actions, &cur, None, &Map::new(), &mut t.rows);
                if id != cur.id {
                    cur = Current { id, ..Default::default() };
                }
                if let Some(u) = usage_dict(&msg["usage"]) {
                    cur.usage = Some(u);
                }
                if let Some(m) = s(msg, "model").filter(|m| !m.starts_with('<')) {
                    let model = format!("claude-code:{m}");
                    t.model = Some(model.clone());
                    cur.model = Some(model);
                }
                cur.at = s(line, "timestamp").map(str::to_string);
            }
            "user" => {
                let at = s(line, "timestamp");
                let content = &line["message"]["content"];
                let blocks = content.as_array().cloned().unwrap_or_default();
                let is_results = blocks.iter().any(|b| b["type"] == "tool_result");
                if is_results {
                    let raw: Map<String, Value> = blocks
                        .iter()
                        .filter(|b| b["type"] == "tool_result" && b["content"].as_array().is_some_and(|c| has_non_text(c)))
                        .filter_map(|b| Some((s(b, "tool_use_id")?.to_string(), b["content"].clone())))
                        .collect();
                    let actions = parser.handle(line);
                    take(actions, &cur, at, &raw, &mut t.rows);
                    continue;
                }
                // A prompt (or an injected note): the assistant row before it
                // must be saved first.
                take(parser.finish(), &cur, None, &Map::new(), &mut t.rows);
                cur = Current::default();
                let text = prompt_text(content);
                let note = line["isMeta"].as_bool().unwrap_or(false) || line["isCompactSummary"].as_bool().unwrap_or(false);
                let mut m = NewMessage::user(text.clone());
                if note {
                    m.kind = Some(db::kind::NOTE.into());
                } else if first_prompt.is_none() && !text.trim().is_empty() && !text.trim_start().starts_with('<') {
                    first_prompt = Some(text.clone());
                }
                if has_non_text(&blocks) {
                    m.extra.get_or_insert_with(Map::new).insert("claude_code_content".into(), content.clone());
                }
                if line["isCompactSummary"].as_bool().unwrap_or(false) {
                    m.extra.get_or_insert_with(Map::new).insert("claude_code_compact_summary".into(), json!(true));
                }
                mark(&mut m, at);
                t.rows.push(m);
            }
            _ => {}
        }
        if t.session_id.is_empty() {
            if let Some(id) = s(line, "sessionId") {
                t.session_id = id.to_string();
            }
        }
        if t.cwd.is_empty() {
            if let Some(cwd) = s(line, "cwd") {
                t.cwd = cwd.to_string();
            }
        }
        if let Some(ts) = s(line, "timestamp") {
            if t.first_at.is_none() {
                t.first_at = Some(ts.to_string());
            }
            t.last_at = Some(ts.to_string());
        }
    }
    take(parser.finish(), &cur, None, &Map::new(), &mut t.rows);
    t.title = custom_title.or(ai_title).or_else(|| first_prompt.map(|p| p.lines().next().unwrap_or("").chars().take(TITLE_CHARS).collect::<String>().trim().to_string()));
    t
}

/// Read and convert one transcript file. Unparseable lines are skipped.
pub fn read_transcript(path: &Path) -> std::io::Result<Transcript> {
    let text = std::fs::read_to_string(path)?;
    let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    Ok(convert_lines(&lines))
}

/// A main transcript and its sub-agent transcripts.
#[derive(Debug, Clone)]
pub struct Found {
    pub path: PathBuf,
    pub subagents: Vec<PathBuf>,
}

fn is_session_stem(stem: &str) -> bool {
    uuid::Uuid::parse_str(stem).is_ok()
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_jsonl(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

fn is_workflow(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "workflows")
}

/// Main transcripts under `root`, newest first, each with its sub-agents
/// (and workflow agents when `workflows`).
pub fn discover(root: &Path, project: Option<&str>, workflows: bool) -> Vec<Found> {
    let mut found = vec![];
    let Ok(projects) = std::fs::read_dir(root) else { return found };
    for p in projects.flatten() {
        let dir = p.path();
        if !dir.is_dir() || project.is_some_and(|f| !p.file_name().to_string_lossy().to_lowercase().contains(&f.to_lowercase())) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        for f in files.flatten() {
            let path = f.path();
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
            if path.extension().is_none_or(|x| x != "jsonl") || !is_session_stem(&stem) {
                continue;
            }
            let mut subagents = vec![];
            let sub_root = dir.join(&stem).join("subagents");
            collect_jsonl(&sub_root, &mut subagents);
            subagents.retain(|p| workflows || !p.strip_prefix(&sub_root).is_ok_and(is_workflow));
            subagents.sort();
            found.push(Found { path, subagents });
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(std::fs::metadata(&f.path).and_then(|m| m.modified()).ok()));
    found
}

/// Stable id for a sub-agent transcript, from its parent and file name.
fn subagent_id(parent: &str, path: &Path) -> uuid::Uuid {
    let name = path.to_string_lossy();
    let tail = name.rsplit_once("/subagents/").map(|(_, t)| t).unwrap_or(&name);
    uuid::Uuid::new_v5(&SUBAGENT_NS, format!("{parent}/{tail}").as_bytes())
}

enum Outcome {
    New(usize),
    Updated(usize),
    Unchanged,
    Skipped,
}

/// Write one transcript as session `id` (under `parent` for a sub-agent).
async fn import_transcript(pool: &DbPool, t: &Transcript, id: &str, parent: Option<&str>, dry_run: bool) -> anyhow::Result<Outcome> {
    let existing = db::get_session(pool, id).await?;
    let mut rows: Vec<&NewMessage> = t.rows.iter().collect();
    let is_new = existing.is_none();
    if !is_new {
        let state = db::claude_import_state(pool, id).await?;
        if state.imported_rows == 0 && state.rows > 0 {
            // OpenAgentd's own session (its live Claude Code turns): never
            // duplicate it from the transcript they also wrote.
            return Ok(Outcome::Skipped);
        }
        if let Some(last) = state.last_imported_at.as_deref().and_then(db::codec::parse_dt) {
            rows.retain(|m| m.created_at.as_deref().and_then(db::codec::parse_dt).is_some_and(|at| at > last));
        }
        if rows.is_empty() {
            return Ok(Outcome::Unchanged);
        }
    }
    if dry_run {
        return Ok(if is_new { Outcome::New(rows.len()) } else { Outcome::Updated(rows.len()) });
    }
    if is_new {
        let uuid = uuid::Uuid::parse_str(id)?;
        db::create_session(
            pool,
            db::NewSession {
                id: Some(uuid),
                parent_session_id: parent.map(str::to_string),
                agent_name: Some(if parent.is_some() { t.agent_name.clone().unwrap_or_else(|| "subagent".into()) } else { AGENT.into() }),
                title: t.title.clone(),
                workspace: t.cwd.clone(),
                model: Some(t.model.clone().unwrap_or_else(|| "claude-code:default".into())),
                ..Default::default()
            },
        )
        .await?;
    }
    let n = rows.len();
    for m in rows {
        db::save_message(pool, id, m.clone()).await?;
    }
    db::set_session_times(pool, id, if is_new { t.first_at.as_deref() } else { None }, t.last_at.as_deref()).await?;
    Ok(if is_new { Outcome::New(n) } else { Outcome::Updated(n) })
}

/// Import everything under `opts.root`.
pub async fn import(pool: &DbPool, opts: &ImportOptions) -> anyhow::Result<ImportReport> {
    let mut report = ImportReport { dry_run: opts.dry_run, ..Default::default() };
    let mut workspaces = std::collections::HashSet::new();
    for f in discover(&opts.root, opts.project.as_deref(), opts.workflows) {
        let mut r = SessionReport { id: f.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), ..Default::default() };
        let t = match read_transcript(&f.path) {
            Ok(t) => t,
            Err(e) => {
                r.status = "error".into();
                r.detail = Some(e.to_string());
                report.sessions.push(r);
                continue;
            }
        };
        if t.cwd.is_empty() || t.rows.is_empty() {
            continue;
        }
        r.title = t.title.clone().unwrap_or_default();
        r.workspace = t.cwd.clone();
        // Transcripts name their own session id; the file name is the fallback.
        let id = if uuid::Uuid::parse_str(&t.session_id).is_ok() { t.session_id.clone() } else { r.id.clone() };
        r.id = id.clone();
        match import_transcript(pool, &t, &id, None, opts.dry_run).await {
            Ok(Outcome::Skipped) => r.status = "skipped".into(),
            Ok(Outcome::Unchanged) => r.status = "unchanged".into(),
            Ok(Outcome::New(n)) => {
                r.status = "new".into();
                r.messages = n;
            }
            Ok(Outcome::Updated(n)) => {
                r.status = "updated".into();
                r.messages = n;
            }
            Err(e) => {
                r.status = "error".into();
                r.detail = Some(e.to_string());
            }
        }
        if matches!(r.status.as_str(), "new" | "updated" | "unchanged") {
            if opts.subagents {
                for sub in &f.subagents {
                    let Ok(st) = read_transcript(sub) else { continue };
                    if st.rows.is_empty() {
                        continue;
                    }
                    let sid = subagent_id(&id, sub).to_string();
                    let mut st = st;
                    if st.cwd.is_empty() {
                        st.cwd = t.cwd.clone();
                    }
                    if st.agent_name.is_none() {
                        st.agent_name = sub.file_stem().map(|s| s.to_string_lossy().to_string());
                    }
                    match import_transcript(pool, &st, &sid, Some(&id), opts.dry_run).await {
                        Ok(Outcome::New(n) | Outcome::Updated(n)) => {
                            r.subagents += 1;
                            report.messages += n;
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("claude_import_subagent_failed path={} err={e}", sub.display()),
                    }
                }
            }
            if !opts.dry_run && Path::new(&t.cwd).is_dir() && !appv3_core::settings().is_chat_workspace(Some(Path::new(&t.cwd))) && workspaces.insert(t.cwd.clone()) {
                db::upsert_coding_workspace(pool, &t.cwd, "repo", None, None, false, false).await?;
            }
        }
        report.messages += r.messages;
        report.subagents += r.subagents;
        report.sessions.push(r);
    }
    report.workspaces =
        report.sessions.iter().filter(|s| matches!(s.status.as_str(), "new" | "updated")).map(|s| s.workspace.clone()).collect::<std::collections::HashSet<_>>().len();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<Value> {
        vec![
            json!({"type":"user","sessionId":"0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e","cwd":"/w","timestamp":"2026-09-01T10:00:00Z","message":{"role":"user","content":"Fix the login bug\nmore"}}),
            json!({"type":"assistant","timestamp":"2026-09-01T10:00:05Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"thinking","thinking":"hmm"}],"usage":{"input_tokens":3,"cache_read_input_tokens":10,"output_tokens":1}}}),
            json!({"type":"assistant","timestamp":"2026-09-01T10:00:06Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/w/a.ts"}}],"usage":{"input_tokens":3,"cache_read_input_tokens":10,"output_tokens":9}}}),
            json!({"type":"user","timestamp":"2026-09-01T10:00:07Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"code"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}]}}),
            json!({"type":"assistant","timestamp":"2026-09-01T10:00:09Z","message":{"id":"m2","model":"claude-opus-5-5","content":[{"type":"text","text":"Fixed."}],"usage":{"input_tokens":5,"output_tokens":2}}}),
            json!({"type":"user","isMeta":true,"timestamp":"2026-09-01T10:01:00Z","message":{"role":"user","content":[{"type":"document","title":"spec.pdf","source":{"type":"base64","media_type":"application/pdf","data":"BBBB"}}]}}),
            json!({"type":"ai-title","aiTitle":"Fix login","sessionId":"0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e"}),
            json!({"type":"file-history-snapshot","messageId":"x"}),
        ]
    }

    #[test]
    fn converts_a_transcript_into_rows() {
        let t = convert_lines(&lines());
        assert_eq!(t.session_id, "0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e");
        assert_eq!(t.cwd, "/w");
        assert_eq!(t.title.as_deref(), Some("Fix login"));
        assert_eq!(t.model.as_deref(), Some("claude-code:claude-opus-5-5"));
        let roles: Vec<_> = t.rows.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant", "user"]);
        let call = &t.rows[1];
        assert_eq!(call.reasoning_content.as_deref(), Some("hmm"));
        assert_eq!(call.tool_calls.as_ref().unwrap()[0]["function"]["name"], "Read");
        let extra = call.extra.as_ref().unwrap();
        assert_eq!(extra["usage"], json!({"input": 13, "output": 9, "cache": 10}));
        assert_eq!((extra["model"].clone(), extra["claude_code"].clone(), extra["claude_code_import"].clone()), (json!("claude-code:claude-opus-5-5"), json!(true), json!(true)));
        assert_eq!(call.created_at.as_deref(), Some("2026-09-01T10:00:06Z"));
        // The tool result keeps its image block.
        let tool = &t.rows[2];
        assert_eq!(tool.content.as_deref(), Some("code\n[image]"));
        assert_eq!(tool.extra.as_ref().unwrap()["claude_code_content"][1]["source"]["data"], "AAAA");
        // The injected document is a hidden note that keeps the file.
        let note = &t.rows[4];
        assert_eq!(note.kind.as_deref(), Some("note"));
        assert_eq!(note.content.as_deref(), Some("[document: spec.pdf]"));
        assert_eq!(note.extra.as_ref().unwrap()["claude_code_content"][0]["source"]["data"], "BBBB");
    }

    #[test]
    fn titles_fall_back_to_the_first_prompt_line() {
        let mut ls = lines();
        ls.retain(|l| l["type"] != "ai-title");
        assert_eq!(convert_lines(&ls).title.as_deref(), Some("Fix the login bug"));
    }

    #[test]
    fn subagent_ids_are_stable() {
        let p = Path::new("/x/proj/abc/subagents/agent-1.jsonl");
        assert_eq!(subagent_id("abc", p), subagent_id("abc", p));
        assert_ne!(subagent_id("abc", p), subagent_id("abd", p));
    }

    #[tokio::test]
    async fn imports_tops_up_and_never_touches_openagentd_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::create_pool(&dir.path().join("t.db")).await.unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("-w");
        std::fs::create_dir_all(proj.join("0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e/subagents")).unwrap();
        let body = |ls: &[Value]| ls.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        let file = proj.join("0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e.jsonl");
        let mut ls = lines();
        std::fs::write(&file, body(&ls[..5])).unwrap();
        std::fs::write(proj.join("0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e/subagents/agent-a1.jsonl"), body(&ls[..2])).unwrap();
        std::fs::create_dir_all(proj.join("0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e/subagents/workflows/wf_1")).unwrap();
        std::fs::write(proj.join("0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e/subagents/workflows/wf_1/agent-w1.jsonl"), body(&ls[..2])).unwrap();
        let opts = ImportOptions { root: root.clone(), project: None, subagents: true, workflows: false, dry_run: false };
        assert_eq!(discover(&root, None, true)[0].subagents.len(), 2);

        let dry = import(&pool, &ImportOptions { dry_run: true, ..opts.clone() }).await.unwrap();
        assert_eq!((dry.sessions[0].status.as_str(), dry.sessions[0].messages), ("new", 4));
        assert!(db::get_session(&pool, "0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e").await.unwrap().is_none());

        let r = import(&pool, &opts).await.unwrap();
        assert_eq!((r.sessions[0].status.as_str(), r.sessions[0].messages, r.sessions[0].subagents), ("new", 4, 1));
        let s = db::get_session(&pool, "0b9c6c5e-6a3a-4b1e-9d1c-3f2a1b0c9d8e").await.unwrap().unwrap();
        assert_eq!((s.title.as_deref(), s.model.as_deref()), (Some("Fix the login bug"), Some("claude-code:claude-opus-5-5")));

        // Again: nothing new.
        assert_eq!(import(&pool, &opts).await.unwrap().sessions[0].status, "unchanged");
        // The transcript grew: only the new line is added.
        ls.truncate(6);
        std::fs::write(&file, body(&ls)).unwrap();
        let r = import(&pool, &opts).await.unwrap();
        assert_eq!((r.sessions[0].status.as_str(), r.sessions[0].messages), ("updated", 1));

        // A session OpenAgentd created itself is never imported over.
        let own = uuid::Uuid::new_v4();
        db::create_session(&pool, db::NewSession { id: Some(own), workspace: "/w".into(), ..Default::default() }).await.unwrap();
        db::save_message(&pool, &own.to_string(), NewMessage::user("hi")).await.unwrap();
        let mut theirs = lines();
        theirs[0]["sessionId"] = json!(own.to_string());
        std::fs::write(proj.join(format!("{own}.jsonl")), body(&theirs[..5])).unwrap();
        let r = import(&pool, &opts).await.unwrap();
        assert!(r.sessions.iter().any(|s| s.id == own.to_string() && s.status == "skipped"), "{:?}", r.sessions);
    }
}
