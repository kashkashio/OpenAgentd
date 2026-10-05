//! `/api/import`: bring other tools' conversations into OpenAgentd (local
//! fork). Reads files on the server's machine, so it sits behind the normal
//! access-key middleware like every other `/api` route.

use crate::error::{ApiError, ApiResult};
use crate::util::json;
use crate::AppState;
use appv3_agent::claude_code_import::{default_root, import, ImportOptions};
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new().route("/claude-code", post(import_claude_code))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ClaudeCodeImport {
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    project: Option<String>,
    #[serde(default = "yes")]
    subagents: bool,
    #[serde(default)]
    workflows: bool,
}

fn yes() -> bool {
    true
}

async fn import_claude_code(State(st): State<AppState>, raw: Bytes) -> ApiResult<Response> {
    let req: ClaudeCodeImport = if raw.is_empty() {
        ClaudeCodeImport { subagents: true, ..Default::default() }
    } else {
        serde_json::from_slice(&raw).map_err(|e| ApiError::unprocessable(format!("Invalid import request: {e}")))?
    };
    let root = default_root().ok_or_else(|| ApiError::unprocessable("No home directory on the server."))?;
    if !root.is_dir() {
        return Err(ApiError::not_found(format!("No Claude Code transcripts at {} on the server.", root.display())));
    }
    let opts =
        ImportOptions { root, project: req.project.filter(|p| !p.trim().is_empty()), subagents: req.subagents, workflows: req.workflows && req.subagents, dry_run: req.dry_run };
    let report = import(&st.pool, &opts).await.map_err(ApiError::internal)?;
    Ok(json(serde_json::to_value(report).map_err(ApiError::internal)?))
}
