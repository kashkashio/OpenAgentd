//! Per-workspace settings kept in the project at `.openagentd/settings.yaml`.
//!
//! The file is optional and every key in it is optional. Today it carries the
//! default model for new sessions in the workspace and options for Claude Code
//! sessions. Keys this module does not know are preserved on save, so a user
//! can keep notes or future settings in the same file.

use appv3_tools::denied;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Location of the settings file, relative to the workspace root.
pub const WORKSPACE_SETTINGS_FILE: &str = ".openagentd/settings.yaml";

/// Model prefix for sessions driven by the locally installed `claude` CLI.
pub const CLAUDE_CODE_PREFIX: &str = "claude-code:";

/// `claude --permission-mode` values accepted in the settings file.
pub const CLAUDE_CODE_PERMISSION_MODES: [&str; 6] = ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"];

/// Permission mode used when the workspace does not set one.
pub const DEFAULT_CLAUDE_CODE_PERMISSION_MODE: &str = "acceptEdits";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceSettings {
    /// `provider:model` for new sessions in this workspace.
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    /// `claude --permission-mode` for Claude Code sessions.
    pub claude_code_permission_mode: Option<String>,
}

impl WorkspaceSettings {
    /// The Claude Code permission mode, falling back to the default.
    pub fn permission_mode(&self) -> &str {
        self.claude_code_permission_mode.as_deref().unwrap_or(DEFAULT_CLAUDE_CODE_PERMISSION_MODE)
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "model": self.model,
            "thinking_level": self.thinking_level,
            "claude_code": { "permission_mode": self.claude_code_permission_mode },
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceSettingsError {
    #[error("Workspace settings path escapes the workspace.")]
    Escapes,
    #[error("Unsupported Claude Code permission mode '{0}'.")]
    PermissionMode(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

fn settings_path(root: &Path) -> (PathBuf, PathBuf) {
    let root = denied::resolve(root);
    (root.join(WORKSPACE_SETTINGS_FILE), root)
}

/// Refuse a location that a symlink carries out of the workspace.
fn check_inside(path: &Path, root: &Path) -> Result<PathBuf, WorkspaceSettingsError> {
    let resolved = denied::resolve(path);
    if !resolved.starts_with(root) {
        return Err(WorkspaceSettingsError::Escapes);
    }
    Ok(resolved)
}

fn read_map(path: &Path) -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else { return Map::new() };
    match appv3_core::pyyaml::safe_load(&text) {
        Ok(Value::Object(map)) => map,
        Ok(Value::Null) => Map::new(),
        Ok(_) => {
            tracing::warn!("workspace_settings_not_a_mapping path={}", path.display());
            Map::new()
        }
        Err(e) => {
            tracing::warn!("workspace_settings_invalid path={} err={e:?}", path.display());
            Map::new()
        }
    }
}

fn str_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Load the workspace's settings. A missing, unreadable, or invalid file, or
/// one a symlink carries outside the workspace, yields the defaults.
pub fn load(workspace: &Path) -> WorkspaceSettings {
    let (path, root) = settings_path(workspace);
    if !path.exists() || check_inside(&path, &root).is_err() {
        return WorkspaceSettings::default();
    }
    let map = read_map(&path);
    let claude = map.get("claude_code").and_then(Value::as_object);
    let permission_mode = claude.and_then(|c| str_field(c, "permission_mode")).filter(|m| CLAUDE_CODE_PERMISSION_MODES.contains(&m.as_str()));
    WorkspaceSettings { model: str_field(&map, "model"), thinking_level: str_field(&map, "thinking_level"), claude_code_permission_mode: permission_mode }
}

fn set_or_remove(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    match value {
        Some(v) => {
            map.insert(key.to_string(), Value::String(v.to_string()));
        }
        None => {
            map.remove(key);
        }
    }
}

/// Write the settings, keeping keys this module does not manage. Clearing
/// every value leaves the file in place with whatever else it holds.
pub fn save(workspace: &Path, settings: &WorkspaceSettings) -> Result<(), WorkspaceSettingsError> {
    if let Some(mode) = &settings.claude_code_permission_mode {
        if !CLAUDE_CODE_PERMISSION_MODES.contains(&mode.as_str()) {
            return Err(WorkspaceSettingsError::PermissionMode(mode.clone()));
        }
    }
    let (path, root) = settings_path(workspace);
    let dir = path.parent().unwrap_or(&root).to_path_buf();
    // Checked before creating anything, so a symlinked `.openagentd` cannot
    // make us create folders elsewhere, and again once the folder exists.
    check_inside(&dir, &root)?;
    std::fs::create_dir_all(&dir)?;
    check_inside(&dir, &root)?;
    if path.exists() {
        check_inside(&path, &root)?;
    }

    let mut map = read_map(&path);
    set_or_remove(&mut map, "model", settings.model.as_deref());
    set_or_remove(&mut map, "thinking_level", settings.thinking_level.as_deref());
    let mut claude = map.get("claude_code").and_then(Value::as_object).cloned().unwrap_or_default();
    set_or_remove(&mut claude, "permission_mode", settings.claude_code_permission_mode.as_deref());
    if claude.is_empty() {
        map.remove("claude_code");
    } else {
        map.insert("claude_code".into(), Value::Object(claude));
    }
    let text = if map.is_empty() { String::new() } else { appv3_core::pyyaml::safe_dump(&Value::Object(map)) };
    appv3_core::secret_files::write_atomic(&path, &text)?;
    Ok(())
}

/// The workspace default model for a new session, if the workspace sets one.
/// The chat workspace never carries project settings.
pub fn default_model_for(workspace: Option<&str>) -> Option<(String, Option<String>)> {
    let ws = workspace.map(str::trim).filter(|w| !w.is_empty())?;
    let path = Path::new(ws);
    if appv3_core::settings::settings().is_chat_workspace(Some(path)) || !path.is_dir() {
        return None;
    }
    let s = load(path);
    s.model.map(|m| (m, s.thinking_level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), WorkspaceSettings::default());
    }

    #[test]
    fn round_trip_keeps_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".openagentd")).unwrap();
        std::fs::write(dir.path().join(WORKSPACE_SETTINGS_FILE), "note: keep me\n").unwrap();
        let s = WorkspaceSettings { model: Some("claude-code:sonnet".into()), thinking_level: None, claude_code_permission_mode: Some("plan".into()) };
        save(dir.path(), &s).unwrap();
        assert_eq!(load(dir.path()), s);
        let text = std::fs::read_to_string(dir.path().join(WORKSPACE_SETTINGS_FILE)).unwrap();
        assert!(text.contains("note: keep me"), "{text}");

        save(dir.path(), &WorkspaceSettings::default()).unwrap();
        assert_eq!(load(dir.path()), WorkspaceSettings::default());
        let text = std::fs::read_to_string(dir.path().join(WORKSPACE_SETTINGS_FILE)).unwrap();
        assert!(!text.contains("claude_code"), "{text}");
    }

    #[test]
    fn rejects_unknown_permission_mode() {
        let dir = tempfile::tempdir().unwrap();
        let s = WorkspaceSettings { claude_code_permission_mode: Some("yolo".into()), ..Default::default() };
        assert!(matches!(save(dir.path(), &s), Err(WorkspaceSettingsError::PermissionMode(_))));
    }

    #[test]
    fn invalid_yaml_is_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".openagentd")).unwrap();
        std::fs::write(dir.path().join(WORKSPACE_SETTINGS_FILE), "model: [unclosed\n").unwrap();
        assert_eq!(load(dir.path()), WorkspaceSettings::default());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folder_outside_workspace_is_refused() {
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.path().join(".openagentd")).unwrap();
        let s = WorkspaceSettings { model: Some("openai:gpt-5".into()), ..Default::default() };
        assert!(matches!(save(ws.path(), &s), Err(WorkspaceSettingsError::Escapes)));
        assert!(!outside.path().join("settings.yaml").exists());
        std::fs::write(outside.path().join("settings.yaml"), "model: openai:gpt-5\n").unwrap();
        assert_eq!(load(ws.path()), WorkspaceSettings::default());
    }
}
