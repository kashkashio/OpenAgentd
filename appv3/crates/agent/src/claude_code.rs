//! Claude Code sessions: a turn whose model is `claude-code:<alias>` runs the
//! locally installed `claude` CLI in the workspace instead of OpenAgentd's own
//! agent loop. The CLI keeps its own login (a Claude subscription or whatever
//! the user configured there); OpenAgentd never reads those credentials.
//!
//! One turn = one `claude -p` process. The OpenAgentd session id doubles as
//! the Claude session id, so later turns `--resume` the same conversation.
//! Output is `--output-format stream-json`; text and thinking stream from the
//! partial-message events, while tool calls, tool results, and the final text
//! are saved as ordinary transcript rows and announced with the existing SSE
//! events, so the UI renders them like any other session.

use crate::errors::AgentError;
use crate::events::{self, Envelope, UsageFrame};
use crate::stream_store::store;
use crate::util::Event;
use crate::workspace_settings::CLAUDE_CODE_PREFIX;
use appv3_db::{self as db, DbPool, NewMessage};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Model aliases offered in the registry (`claude --model <alias>`); each
/// follows the CLI to the latest model of its family.
pub const MODEL_ALIASES: [&str; 5] = ["default", "sonnet", "opus", "haiku", "opusplan"];

/// How many specific Claude models (newest first) the picker offers.
const PINNED_MODEL_LIMIT: usize = 12;

/// The registry's `claude-code:*` models: the aliases, then specific Claude
/// models (`claude --model claude-opus-5-5`) from the Anthropic catalog.
pub fn model_choices(anthropic_newest_first: &[String]) -> Vec<String> {
    let pinned = anthropic_newest_first.iter().filter(|m| m.starts_with("claude-")).take(PINNED_MODEL_LIMIT).cloned();
    MODEL_ALIASES.iter().map(|m| m.to_string()).chain(pinned).collect()
}

/// Environment variables removed from the CLI's environment: OpenAgentd's own
/// secrets, plus API credentials that would make the CLI bill an API key
/// instead of the user's login.
const SCRUBBED_ENV: [&str; 5] = ["OPENAGENTD_DESKTOP_TOKEN", "OPENAGENTD_ACCESS_KEY", "OPENAGENTD_HANDSHAKE_FILE", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

const TERM_GRACE: Duration = Duration::from_secs(2);
const STDERR_LIMIT: usize = 8 * 1024;

/// The alias of a `claude-code:<alias>` model id.
pub fn alias(model: &str) -> Option<&str> {
    model.strip_prefix(CLAUDE_CODE_PREFIX).map(str::trim).filter(|a| !a.is_empty())
}

/// Locate the `claude` binary: `OPENAGENTD_CLAUDE_BIN`, then `PATH`, then the
/// install locations a GUI-launched app (whose `PATH` is minimal) would miss.
pub fn find_cli() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OPENAGENTD_CLAUDE_BIN").map(PathBuf::from).filter(|p| appv3_core::which::is_executable(p)) {
        return Some(p);
    }
    if let Some(p) = appv3_core::which::which("claude") {
        return Some(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    [home.join(".local/bin/claude"), home.join(".claude/local/claude"), "/opt/homebrew/bin/claude".into(), "/usr/local/bin/claude".into()]
        .into_iter()
        .find(|p| appv3_core::which::is_executable(p))
}

/// Everything a Claude Code turn needs from the session.
pub struct TurnContext<'a> {
    pub pool: &'a DbPool,
    pub session_id: String,
    pub workspace: String,
    pub agent_name: String,
    pub model: String,
    pub permission_mode: String,
    pub cancel: &'a Event,
    pub hard_cancel: &'a Event,
}

/// What the stream parser asks the runner to do.
#[derive(Debug)]
pub enum Action {
    Emit(Envelope),
    Save(NewMessage),
    /// The CLI's final `result` line.
    Finished {
        usage: UsageFrame,
        error: Option<String>,
    },
}

#[derive(Default)]
struct PendingAssistant {
    id: String,
    text: String,
    thinking: String,
    tool_calls: Vec<Value>,
    /// This model call's tokens, in the `extra.usage` shape the transcript
    /// footer and session totals read (no `cost`: a login is not billed).
    usage: Option<Value>,
    /// Set on the turn's last row from the CLI's `result`.
    turn: Option<(Option<f64>, Option<f64>)>,
}

/// Turns stream-json lines into transcript rows and SSE events.
pub struct StreamParser {
    agent: String,
    model: String,
    pending: Option<PendingAssistant>,
    /// tool_use id → tool name, for the matching tool_result.
    tools: std::collections::HashMap<String, String>,
    /// Text streamed since the last complete text block; saved if the turn
    /// is interrupted before the CLI reports the block.
    streamed: String,
    /// How the CLI authenticated (`apiKeySource` from its `init` event):
    /// `none` is the user's Claude login, anything else names a key source.
    auth: Option<String>,
    /// The model the CLI actually runs (`claude-opus-5-5` for alias `opus`),
    /// from its `init` event; replies record it so the footer shows the
    /// real version.
    resolved: Option<String>,
    /// Prompt tokens (input + cache) of the model call in progress, from its
    /// `message_start`; reported with its output once `message_delta` ends it.
    call_prompt: i64,
    call_cached: Option<i64>,
    /// Usage went out per model call, so the final `result` total is not sent.
    usage_sent: bool,
    emitted: bool,
}

impl StreamParser {
    pub fn new(agent: &str, model: &str) -> Self {
        Self {
            agent: agent.into(),
            model: model.into(),
            pending: None,
            tools: Default::default(),
            streamed: String::new(),
            auth: None,
            resolved: None,
            call_prompt: 0,
            call_cached: None,
            usage_sent: false,
            emitted: false,
        }
    }

    /// The CLI's auth source, once its `init` event arrived.
    pub fn auth(&self) -> Option<&str> {
        self.auth.as_deref()
    }

    /// Whether usage was already reported per model call.
    pub fn usage_sent(&self) -> bool {
        self.usage_sent
    }

    /// Whether any user-visible output has been produced yet.
    pub fn emitted(&self) -> bool {
        self.emitted
    }

    /// `claude-code:<model the CLI runs>`, or the session's model until known.
    pub fn display_model(&self) -> String {
        match &self.resolved {
            Some(m) => format!("{}{m}", crate::workspace_settings::CLAUDE_CODE_PREFIX),
            None => self.model.clone(),
        }
    }

    fn meta(&self) -> Value {
        json!({"model": self.display_model()})
    }

    fn flush(&mut self, out: &mut Vec<Action>) {
        let Some(p) = self.pending.take() else { return };
        if p.text.is_empty() && p.thinking.is_empty() && p.tool_calls.is_empty() {
            return;
        }
        let mut msg = NewMessage::assistant(Some(p.text).filter(|t| !t.is_empty()));
        msg.reasoning_content = Some(p.thinking).filter(|t| !t.is_empty());
        if !p.tool_calls.is_empty() {
            msg.tool_calls = Some(Value::Array(p.tool_calls));
        }
        let mut extra = Map::new();
        extra.insert("model".into(), json!(self.display_model()));
        extra.insert("claude_code".into(), json!(true));
        if let Some(a) = &self.auth {
            extra.insert("claude_auth".into(), json!(a));
        }
        if let Some(u) = p.usage {
            extra.insert("usage".into(), u);
        }
        if let Some((duration_ms, api_cost)) = p.turn {
            if let Some(d) = duration_ms {
                extra.insert("duration_ms".into(), json!(d));
            }
            // What the turn would have cost on the API. Kept out of `usage`
            // so it never counts as spend.
            if let Some(c) = api_cost {
                extra.insert("claude_code_api_cost_usd".into(), json!(c));
            }
        }
        msg.extra = Some(extra);
        out.push(Action::Save(msg));
    }

    /// Feed one parsed stream-json line.
    pub fn handle(&mut self, line: &Value) -> Vec<Action> {
        let mut out = vec![];
        // Subagent (Task tool) traffic carries a parent id; its result reaches
        // the transcript through the Task tool_result, not as the reply.
        if line.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
            return out;
        }
        match line.get("type").and_then(Value::as_str).unwrap_or("") {
            "system" if line["subtype"] == "init" => {
                self.auth = line["apiKeySource"].as_str().map(str::to_string);
                self.resolved = line["model"].as_str().filter(|m| !m.is_empty()).map(str::to_string);
            }
            "stream_event" => {
                let ev = &line["event"];
                if ev["type"] == "message_start" {
                    let u = &ev["message"]["usage"];
                    let n = |k: &str| u[k].as_i64().unwrap_or(0);
                    self.call_prompt = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                    self.call_cached = u["cache_read_input_tokens"].as_i64();
                    if let Some(m) = ev["message"]["model"].as_str().filter(|m| !m.is_empty()) {
                        self.resolved = Some(m.to_string());
                    }
                } else if ev["type"] == "message_delta" {
                    // Live usage, one frame per model call, so the working row
                    // can count tokens while the turn runs.
                    if let Some(output) = ev["usage"]["output_tokens"].as_i64() {
                        self.usage_sent = true;
                        if let Some(p) = self.pending.as_mut() {
                            let mut u = json!({"input": self.call_prompt, "output": output});
                            if let Some(c) = self.call_cached {
                                u["cache"] = json!(c);
                            }
                            p.usage = Some(u);
                        }
                        let frame = UsageFrame {
                            prompt_tokens: self.call_prompt,
                            completion_tokens: output,
                            total_tokens: self.call_prompt + output,
                            cached_tokens: self.call_cached,
                            ..Default::default()
                        };
                        out.push(Action::Emit(events::usage(&frame, json!({"agent": self.agent, "model": self.display_model()}))));
                    }
                } else if ev["type"] == "content_block_delta" {
                    let d = &ev["delta"];
                    match d["type"].as_str() {
                        Some("text_delta") => {
                            if let Some(t) = d["text"].as_str().filter(|t| !t.is_empty()) {
                                self.emitted = true;
                                self.streamed.push_str(t);
                                out.push(Action::Emit(events::message(&self.agent, t, Some(self.meta()))));
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(t) = d["thinking"].as_str().filter(|t| !t.is_empty()) {
                                self.emitted = true;
                                out.push(Action::Emit(events::thinking(&self.agent, t, Some(self.meta()))));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "assistant" => {
                let msg = &line["message"];
                let id = msg["id"].as_str().unwrap_or("").to_string();
                if self.pending.as_ref().is_some_and(|p| p.id != id) {
                    self.flush(&mut out);
                }
                let mut p = self.pending.take().unwrap_or_else(|| PendingAssistant { id: id.clone(), ..Default::default() });
                for block in msg["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            p.text.push_str(block["text"].as_str().unwrap_or(""));
                            self.streamed.clear();
                        }
                        Some("thinking") => p.thinking.push_str(block["thinking"].as_str().unwrap_or("")),
                        Some("tool_use") => {
                            let call_id = block["id"].as_str().unwrap_or("").to_string();
                            let name = block["name"].as_str().unwrap_or("tool").to_string();
                            let args = events::compact(&block["input"]);
                            self.tools.insert(call_id.clone(), name.clone());
                            self.emitted = true;
                            out.push(Action::Emit(events::tool_call(&self.agent, Some(&call_id), &name)));
                            out.push(Action::Emit(events::tool_start(&self.agent, Some(&call_id), &name, Some(&args))));
                            p.tool_calls.push(json!({"id": call_id, "type": "function", "function": {"name": name, "arguments": args}}));
                        }
                        _ => {}
                    }
                }
                self.pending = Some(p);
            }
            "user" => {
                let results: Vec<&Value> = line["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result").collect();
                if results.is_empty() {
                    return out;
                }
                // The assistant row holding the calls must precede their results.
                self.flush(&mut out);
                for r in results {
                    let call_id = r["tool_use_id"].as_str().unwrap_or("").to_string();
                    let name = self.tools.get(&call_id).cloned().unwrap_or_else(|| "tool".into());
                    let text = tool_result_text(&r["content"]);
                    let is_error = r["is_error"].as_bool().unwrap_or(false);
                    out.push(Action::Save(NewMessage::tool(&call_id, &name, &text)));
                    let metadata = if is_error { Some(json!({"is_error": true})) } else { None };
                    out.push(Action::Emit(events::tool_end(&self.agent, Some(&call_id), &name, Some(&text), metadata)));
                }
            }
            "result" => {
                if let Some(p) = self.pending.as_mut() {
                    p.turn = Some((line["duration_ms"].as_f64(), line["total_cost_usd"].as_f64()));
                }
                self.flush(&mut out);
                let u = &line["usage"];
                let n = |k: &str| u[k].as_i64().unwrap_or(0);
                let prompt = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                let completion = n("output_tokens");
                let usage = UsageFrame {
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    total_tokens: prompt + completion,
                    cached_tokens: u["cache_read_input_tokens"].as_i64(),
                    thoughts_tokens: u["output_tokens_details"]["thinking_tokens"].as_i64(),
                    tool_use_tokens: None,
                    // The CLI's figure is list price; a subscription login is
                    // not billed per token, so no cost is reported.
                    estimated_cost_usd: None,
                };
                let error = if line["is_error"].as_bool().unwrap_or(false) {
                    let detail = line["result"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .or_else(|| line["errors"].as_array().map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).filter(|s| !s.is_empty()));
                    Some(detail.unwrap_or_else(|| format!("Claude Code ended with {}.", line["subtype"].as_str().unwrap_or("an error"))))
                } else {
                    None
                };
                out.push(Action::Finished { usage, error });
            }
            _ => {}
        }
        out
    }

    /// Save whatever is still buffered (an interrupted turn).
    pub fn finish(&mut self) -> Vec<Action> {
        let mut out = vec![];
        let partial = std::mem::take(&mut self.streamed);
        if !partial.is_empty() {
            self.pending.get_or_insert_with(Default::default).text.push_str(&partial);
        }
        self.flush(&mut out);
        out
    }
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| match p["type"].as_str() {
                Some("text") => p["text"].as_str().map(str::to_string),
                Some("image") => Some("[image]".into()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => events::compact(other),
    }
}

fn row_is_claude_code(row: &db::SessionMessage) -> bool {
    row.role == "assistant" && row.extra.as_deref().and_then(|e| serde_json::from_str::<Value>(e).ok()).is_some_and(|e| e["claude_code"] == json!(true))
}

/// The prompt for this turn (the latest user message) and whether Claude
/// already holds this conversation.
/// Files the user attached to a message (`extra.attachments`, saved under
/// the session's `uploads/`), as a note the CLI can act on: Claude Code reads
/// PDFs, images and text with its own Read tool, given absolute paths.
fn attachments_note(extra: Option<&str>) -> Option<String> {
    let extra: Value = serde_json::from_str(extra?).ok()?;
    let lines: Vec<String> = extra["attachments"]
        .as_array()?
        .iter()
        .filter_map(|a| {
            let path = a["path"].as_str().or_else(|| a["workspace_path"].as_str())?;
            let name = a["original_name"].as_str().or_else(|| a["filename"].as_str()).unwrap_or("file");
            let kind = a["media_type"].as_str().map(|m| format!(", {m}")).unwrap_or_default();
            Some(format!("- {path} ({name}{kind})"))
        })
        .collect();
    (!lines.is_empty()).then(|| format!("Attached files (read them with the Read tool):\n{}", lines.join("\n")))
}

/// The prompt for this turn (the latest user message, plus any files it
/// attached) and whether Claude already holds this conversation.
async fn turn_input(pool: &DbPool, session_id: &str) -> Result<(String, bool), AgentError> {
    let rows = db::llm_window_rows(pool, session_id, true).await.map_err(|e| AgentError::Other(e.to_string()))?;
    let resume = rows.iter().any(row_is_claude_code);
    let latest = rows.iter().rev().find(|r| r.role == "user" && r.kind != "note").or_else(|| rows.iter().rev().find(|r| r.role == "user"));
    let text = latest.and_then(|r| r.content.clone()).unwrap_or_default();
    let note = latest.and_then(|r| attachments_note(r.extra.as_deref()));
    let prompt = match note {
        Some(n) if text.trim().is_empty() => n,
        Some(n) => format!("{text}\n\n{n}"),
        None => text,
    };
    if prompt.trim().is_empty() {
        return Err(AgentError::Other("Nothing to send to Claude Code.".into()));
    }
    Ok((prompt, resume))
}

enum Attempt {
    Done,
    /// The CLI refused the session flag before producing output; retry with
    /// the other one (`--session-id` ↔ `--resume`).
    SwitchSessionFlag,
}

/// Run one turn through the `claude` CLI.
pub async fn run_turn(ctx: TurnContext<'_>) -> Result<(), AgentError> {
    let alias = alias(&ctx.model).ok_or_else(|| AgentError::Unconfigured(format!("Not a Claude Code model: {}", ctx.model)))?.to_string();
    let bin =
        find_cli().ok_or_else(|| AgentError::Unconfigured("Claude Code CLI not found. Install it (https://claude.com/claude-code) and run `claude` once to sign in.".into()))?;
    let claude_session = db::codec::parse_uuid(&ctx.session_id).map(|u| u.hyphenated().to_string()).unwrap_or_else(|| ctx.session_id.clone());
    let (prompt, mut resume) = turn_input(ctx.pool, &ctx.session_id).await?;
    for _ in 0..2 {
        match run_once(&ctx, &bin, &alias, &claude_session, resume, &prompt).await? {
            Attempt::Done => return Ok(()),
            Attempt::SwitchSessionFlag => resume = !resume,
        }
    }
    Err(AgentError::Other("Claude Code could not open this conversation.".into()))
}

async fn run_once(ctx: &TurnContext<'_>, bin: &PathBuf, alias: &str, claude_session: &str, resume: bool, prompt: &str) -> Result<Attempt, AgentError> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]);
    if alias != "default" {
        cmd.args(["--model", alias]);
    }
    cmd.args(["--permission-mode", &ctx.permission_mode]);
    cmd.args([if resume { "--resume" } else { "--session-id" }, claude_session]);
    if !ctx.workspace.is_empty() {
        cmd.current_dir(&ctx.workspace);
    }
    for k in SCRUBBED_ENV {
        cmd.env_remove(k);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    appv3_core::proctree::configure(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| AgentError::Other(format!("Could not start Claude Code ({}): {e}", bin.display())))?;
    let tree = appv3_core::proctree::ProcessTree::attach(&child);

    if let Some(mut stdin) = child.stdin.take() {
        let text = prompt.to_string();
        tokio::spawn(async move {
            let _ = stdin.write_all(text.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let stderr = child.stderr.take().map(|mut err| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while let Ok(n) = err.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                if buf.len() < STDERR_LIMIT {
                    buf.extend_from_slice(&chunk[..n.min(STDERR_LIMIT - buf.len())]);
                }
            }
            String::from_utf8_lossy(&buf).trim().to_string()
        })
    });
    let stdout = child.stdout.take().ok_or_else(|| AgentError::Other("Claude Code produced no output stream.".into()))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut parser = StreamParser::new(&ctx.agent_name, &ctx.model);
    let mut finished: Option<(UsageFrame, Option<String>)> = None;
    let mut cancelled = false;

    loop {
        let next = tokio::select! {
            l = lines.next_line() => l,
            _ = ctx.cancel.wait() => { cancelled = true; break; }
            _ = ctx.hard_cancel.wait() => { cancelled = true; break; }
        };
        let Ok(Some(line)) = next else { break };
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        for action in parser.handle(&v) {
            match action {
                Action::Emit(env) => store().push_event(&ctx.session_id, &env, false),
                Action::Save(msg) => save(ctx, msg).await?,
                Action::Finished { usage, error } => finished = Some((usage, error)),
            }
        }
    }

    if cancelled {
        tree.terminate(&mut child, TERM_GRACE).await;
        for action in parser.finish() {
            if let Action::Save(msg) = action {
                save(ctx, msg).await?;
            }
        }
        return Ok(Attempt::Done);
    }
    let status = child.wait().await.ok();
    let stderr = match stderr {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    for action in parser.finish() {
        if let Action::Save(msg) = action {
            save(ctx, msg).await?;
        }
    }

    if !parser.emitted() {
        let flag_refused = if resume { stderr.contains("No conversation found") } else { stderr.contains("already in use") };
        if flag_refused {
            return Ok(Attempt::SwitchSessionFlag);
        }
    }
    tracing::info!("claude_code_turn session_id={} model={} auth={} resume={}", ctx.session_id, ctx.model, parser.auth().unwrap_or("unknown"), resume);
    match finished {
        Some((usage, error)) => {
            if !parser.usage_sent() {
                store().push_event(&ctx.session_id, &events::usage(&usage, json!({"agent": ctx.agent_name, "model": ctx.model})), false);
            }
            match error {
                Some(e) => Err(AgentError::Other(with_login_hint(e))),
                None => Ok(Attempt::Done),
            }
        }
        None => {
            let code = status.and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
            let detail = if stderr.is_empty() { format!("Claude Code exited ({code}) without a result.") } else { stderr };
            Err(AgentError::Other(with_login_hint(detail)))
        }
    }
}

/// The CLI's own login (not the desktop app's or an IDE extension's) is
/// what a server-run turn uses; say how to fix it when it is missing.
pub(crate) fn with_login_hint(error: String) -> String {
    let lower = error.to_ascii_lowercase();
    let login = ["not logged in", "oauth", "failed to authenticate", "/login", "invalid api key", "authentication"].iter().any(|k| lower.contains(k));
    if !login {
        return error;
    }
    format!(
        "{error}\n\nThe `claude` CLI on the OpenAgentd server is not signed in (the Claude desktop app and IDE extensions keep their own logins). On that machine, run `claude auth login` in a terminal, check with `claude auth status`, then retry."
    )
}

async fn save(ctx: &TurnContext<'_>, msg: NewMessage) -> Result<(), AgentError> {
    db::save_message(ctx.pool, &ctx.session_id, msg).await.map_err(|e| AgentError::Other(e.to_string()))?;
    store().commit_agent_content(&ctx.session_id, &ctx.agent_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(p: &mut StreamParser, lines: &[Value]) -> Vec<Action> {
        lines.iter().flat_map(|l| p.handle(l)).collect()
    }

    fn event_names(actions: &[Action]) -> Vec<String> {
        actions
            .iter()
            .map(|a| match a {
                Action::Emit(e) => format!("emit:{}", e.event),
                Action::Save(m) => format!("save:{}", m.role),
                Action::Finished { error, .. } => format!("finished:{}", error.is_some()),
            })
            .collect()
    }

    #[test]
    fn model_choices_lists_aliases_then_newest_claude_models() {
        let catalog = ["claude-opus-5-5".to_string(), "claude-sonnet-5-5".to_string(), "not-claude".to_string()];
        assert_eq!(model_choices(&catalog), ["default", "sonnet", "opus", "haiku", "opusplan", "claude-opus-5-5", "claude-sonnet-5-5"]);
    }

    #[test]
    fn replies_record_the_model_the_cli_runs() {
        let mut p = StreamParser::new("code", "claude-code:opus");
        let actions = feed(
            &mut p,
            &[
                json!({"type":"system","subtype":"init","apiKeySource":"none","model":"claude-opus-5-5"}),
                json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"hi"}]}}),
            ],
        );
        assert!(actions.is_empty());
        let tail = p.finish();
        let Action::Save(row) = &tail[0] else { panic!() };
        assert_eq!(row.extra.as_ref().unwrap()["model"], json!("claude-code:claude-opus-5-5"));
    }

    #[test]
    fn attached_files_are_named_for_the_cli() {
        let extra = r#"{"attachments":[{"filename":"spec.pdf","path":"/w/uploads/spec.pdf","original_name":"BO1 Spec.pdf","media_type":"application/pdf"}]}"#;
        assert_eq!(attachments_note(Some(extra)).as_deref(), Some("Attached files (read them with the Read tool):\n- /w/uploads/spec.pdf (BO1 Spec.pdf, application/pdf)"));
        assert_eq!(attachments_note(Some(r#"{"model":"x"}"#)), None);
        assert_eq!(attachments_note(None), None);
    }

    #[test]
    fn login_failures_say_how_to_sign_in() {
        let e = with_login_hint("Failed to authenticate: OAuth session expired and could not be refreshed".into());
        assert!(e.contains("claude auth login"), "{e}");
        assert_eq!(with_login_hint("Rate limited".into()), "Rate limited");
    }

    #[test]
    fn alias_parses_prefix() {
        assert_eq!(alias("claude-code:sonnet"), Some("sonnet"));
        assert_eq!(alias("claude-code:"), None);
        assert_eq!(alias("anthropic:claude"), None);
    }

    #[test]
    fn text_streams_then_saves_one_assistant_row() {
        let mut p = StreamParser::new("code", "claude-code:sonnet");
        let actions = feed(
            &mut p,
            &[
                json!({"type":"system","subtype":"init","apiKeySource":"none"}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}}),
                json!({"type":"assistant","message":{"id":"m1","content":[{"type":"thinking","thinking":"hmm"}]},"parent_tool_use_id":null}),
                json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}),
                json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"hi"}]},"parent_tool_use_id":null}),
                json!({"type":"result","is_error":false,"usage":{"input_tokens":2,"cache_read_input_tokens":10,"output_tokens":4}}),
            ],
        );
        assert_eq!(event_names(&actions), ["emit:thinking", "emit:message", "save:assistant", "finished:false"]);
        let Action::Save(row) = &actions[2] else { panic!() };
        assert_eq!(row.content.as_deref(), Some("hi"));
        assert_eq!(row.reasoning_content.as_deref(), Some("hmm"));
        assert_eq!(row.extra.as_ref().unwrap()["claude_code"], json!(true));
        assert_eq!(row.extra.as_ref().unwrap()["claude_auth"], json!("none"));
        let Action::Finished { usage, .. } = &actions[3] else { panic!() };
        assert_eq!((usage.prompt_tokens, usage.completion_tokens, usage.cached_tokens), (12, 4, Some(10)));
    }

    #[test]
    fn tool_use_and_result_pair_up() {
        let mut p = StreamParser::new("code", "claude-code:sonnet");
        let actions = feed(
            &mut p,
            &[
                json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Looking."}]}}),
                json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}),
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"README.md"}]}]}}),
                json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Done."}]}}),
            ],
        );
        assert_eq!(event_names(&actions), ["emit:tool_call", "emit:tool_start", "save:assistant", "save:tool", "emit:tool_end"]);
        let Action::Save(call) = &actions[2] else { panic!() };
        assert_eq!(call.tool_calls.as_ref().unwrap()[0]["function"]["name"], "Bash");
        assert_eq!(call.content.as_deref(), Some("Looking."));
        let Action::Save(result) = &actions[3] else { panic!() };
        assert_eq!((result.tool_call_id.as_deref(), result.name.as_deref(), result.content.as_deref()), (Some("t1"), Some("Bash"), Some("README.md")));
        let tail = p.finish();
        assert_eq!(event_names(&tail), ["save:assistant"]);
    }

    #[test]
    fn usage_streams_per_model_call() {
        let mut p = StreamParser::new("code", "claude-code:sonnet");
        let actions = feed(
            &mut p,
            &[
                json!({"type":"stream_event","event":{"type":"message_start","message":{"usage":{"input_tokens":3,"cache_read_input_tokens":100}}}}),
                json!({"type":"stream_event","event":{"type":"message_delta","usage":{"output_tokens":42}}}),
                json!({"type":"stream_event","parent_tool_use_id":"t1","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"inner"}}}),
            ],
        );
        assert_eq!(event_names(&actions), ["emit:usage"]);
        let Action::Emit(e) = &actions[0] else { panic!() };
        assert_eq!((e.field("prompt_tokens"), e.field("completion_tokens"), e.field("cached_tokens")), (Some(&json!(103)), Some(&json!(42)), Some(&json!(100))));
        assert!(p.usage_sent());
    }

    #[test]
    fn rows_carry_usage_and_the_turn_totals() {
        let mut p = StreamParser::new("code", "claude-code:sonnet");
        let actions = feed(
            &mut p,
            &[
                json!({"type":"stream_event","event":{"type":"message_start","message":{"usage":{"input_tokens":5,"cache_read_input_tokens":10}}}}),
                json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"hi"}]}}),
                json!({"type":"stream_event","event":{"type":"message_delta","usage":{"output_tokens":7}}}),
                json!({"type":"result","is_error":false,"duration_ms":16000.0,"total_cost_usd":0.24,"usage":{}}),
            ],
        );
        let row = actions
            .iter()
            .find_map(|a| match a {
                Action::Save(m) => Some(m),
                _ => None,
            })
            .unwrap();
        let extra = row.extra.as_ref().unwrap();
        assert_eq!(extra["usage"], json!({"input": 15, "output": 7, "cache": 10}));
        assert_eq!((extra["duration_ms"].clone(), extra["claude_code_api_cost_usd"].clone()), (json!(16000.0), json!(0.24)));
        assert!(extra["usage"].get("cost").is_none());
    }

    #[test]
    fn interrupted_text_is_kept() {
        let mut p = StreamParser::new("code", "claude-code:sonnet");
        feed(&mut p, &[json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"1, 2, "}}})]);
        let tail = p.finish();
        let Action::Save(row) = &tail[0] else { panic!() };
        assert_eq!(row.content.as_deref(), Some("1, 2, "));
    }

    #[test]
    fn subagent_traffic_is_skipped() {
        let mut p = StreamParser::new("code", "claude-code:sonnet");
        let actions = feed(&mut p, &[json!({"type":"assistant","parent_tool_use_id":"t9","message":{"id":"x","content":[{"type":"text","text":"inner"}]}})]);
        assert!(actions.is_empty());
        assert!(p.finish().is_empty());
    }

    #[test]
    fn error_result_carries_message() {
        let mut p = StreamParser::new("code", "claude-code:sonnet");
        let actions = feed(&mut p, &[json!({"type":"result","is_error":true,"subtype":"success","result":"Not logged in · Please run /login"})]);
        let Action::Finished { error, .. } = &actions[0] else { panic!() };
        assert_eq!(error.as_deref(), Some("Not logged in · Please run /login"));
    }
}
