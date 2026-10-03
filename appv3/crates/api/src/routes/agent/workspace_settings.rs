//! `GET/PUT /api/agent/workspace/settings` — the per-project
//! `.openagentd/settings.yaml` (v3-only; see appv3/REPORT.md).

use super::helpers::{validate_model_settings, validate_workspace_or_422};
use crate::error::{ApiError, ApiResult};
use crate::util::*;
use crate::AppState;
use appv3_agent::workspace_settings::{self as ws_settings, WorkspaceSettings, WorkspaceSettingsError};
use appv3_core::settings;
use appv3_db as db;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use bytes::Bytes;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn router() -> Router<AppState> {
    Router::new().route("/workspace/settings", get(get_settings).put(put_settings))
}

fn project_workspace(raw: &str) -> ApiResult<PathBuf> {
    let ws = validate_workspace_or_422(raw, true)?;
    let path = PathBuf::from(&ws);
    if settings().is_chat_workspace(Some(Path::new(&ws))) {
        return Err(ApiError::unprocessable("The chat workspace has no project settings."));
    }
    Ok(path)
}

fn response(workspace: &Path, s: &WorkspaceSettings) -> Response {
    let mut v = s.to_json();
    v["workspace"] = json!(workspace.display().to_string());
    v["path"] = json!(workspace.join(ws_settings::WORKSPACE_SETTINGS_FILE).display().to_string());
    json(v)
}

async fn get_settings(q: Qs) -> ApiResult<Response> {
    let ws = project_workspace(&q.req("workspace")?)?;
    blocking(move || {
        let s = ws_settings::load(&ws);
        Ok(response(&ws, &s))
    })
    .await
}

async fn put_settings(State(st): State<AppState>, raw: Bytes) -> ApiResult<Response> {
    let b = body_value(&raw)?;
    if !b.is_object() {
        return Err(ApiError::unprocessable("Expected a JSON object."));
    }
    let ws = project_workspace(&opt_str_field(&b, "workspace")?.unwrap_or_default())?;
    let model = opt_str_field(&b, "model")?;
    let thinking = opt_str_field(&b, "thinking_level")?;
    let (model, thinking) = validate_model_settings(model.as_deref(), thinking.as_deref())?;
    let permission_mode = match b.get("claude_code") {
        None | Some(Value::Null) => None,
        Some(c @ Value::Object(_)) => opt_str_field(c, "permission_mode")?.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        Some(_) => return Err(ApiError::unprocessable("claude_code must be an object.")),
    };
    // Also switch the workspace's existing sessions, so the project setting
    // covers chats started before it was set. Clearing the default leaves
    // sessions on whatever they use now.
    let apply = opt_bool_field(&b, "apply_to_sessions")?.unwrap_or(false);
    let next = WorkspaceSettings { model, thinking_level: thinking, claude_code_permission_mode: permission_mode };
    let session_model = next.model.clone().filter(|_| apply).map(|m| (m, next.thinking_level.clone()));
    let ws_for_save = ws.clone();
    let saved = blocking(move || {
        ws_settings::save(&ws_for_save, &next).map_err(|e| match e {
            WorkspaceSettingsError::Io(io) => ApiError::new(500, format!("Could not write workspace settings: {io}")),
            other => ApiError::unprocessable(other.to_string()),
        })?;
        Ok::<_, ApiError>(ws_settings::load(&ws_for_save))
    })
    .await?;
    let mut updated = 0;
    if let Some((model, thinking)) = session_model {
        updated = db::set_workspace_session_models(&st.pool, &ws.display().to_string(), &model, thinking.as_deref()).await?;
    }
    let mut v = saved.to_json();
    v["workspace"] = json!(ws.display().to_string());
    v["path"] = json!(ws.join(ws_settings::WORKSPACE_SETTINGS_FILE).display().to_string());
    v["sessions_updated"] = json!(updated);
    Ok(json(v))
}
