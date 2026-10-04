//! `/api/preview`: start, list, and close built-in web previews (v3 only).
//!
//! Each preview is a listener owned by `appv3-preview`; these routes
//! validate the workspace and target, shape the response, and grant the
//! calling machine access when it is not this one (the call itself is
//! authenticated by the access-key middleware).

use crate::error::{ApiError, ApiResult};
use crate::middleware::ConnInfo;
use crate::routes::agent::helpers::validate_workspace_or_422;
use crate::util::{json, no_content, Qs};
use crate::AppState;
use appv3_preview::{parse_url_target, resolve_workspace_file, static_backend, url_path, Backend, PreviewError, PreviewInfo};
use axum::extract::{ConnectInfo, Path as AxPath};
use axum::http::{header, request::Parts};
use axum::response::Response;
use axum::routing::{delete, get};
use axum::Router;
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::IpAddr;
use std::path::Path;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list_previews).post(open_preview)).route("/{id}", delete(close_preview))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRequest {
    workspace: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    preferred_port: Option<u16>,
}

fn preview_json(info: &PreviewInfo, path: &str) -> Value {
    let errors = appv3_preview::global().get(&info.id).map(|e| e.console_error_count()).unwrap_or(0);
    json!({
        "id": info.id,
        "workspace": info.workspace,
        "kind": info.kind,
        "target": info.target,
        "port": info.port,
        "origin": info.origin,
        "path": path,
        "url": format!("{}{}", info.origin, path),
        "console_errors": errors,
    })
}

/// Who is asking, as the preview sees it.
struct Caller {
    /// Another machine's address; `None` for a loopback caller.
    peer: Option<IpAddr>,
    /// The host the preview origin should use: an IP literal the caller can
    /// reach (the preview refuses domain-name hosts, against DNS rebinding).
    host: Option<String>,
    /// The app origin the caller frames the preview from.
    framer: Option<String>,
}

fn caller(parts: &Parts) -> Caller {
    let conn = parts.extensions.get::<ConnectInfo<ConnInfo>>().map(|c| c.0);
    let peer = conn.and_then(|c| c.remote).map(|a| a.ip().to_canonical()).filter(|ip| !ip.is_loopback());
    if peer.is_none() {
        return Caller { peer: None, host: None, framer: None };
    }
    let header_host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok()).and_then(|h| {
        let name = h.rsplit_once(':').map(|(n, _)| n).unwrap_or(h);
        let bare = name.trim_start_matches('[').trim_end_matches(']');
        bare.parse::<IpAddr>().ok().filter(|ip| !ip.is_loopback() && !ip.is_unspecified()).map(|_| name.to_string())
    });
    // Reached by name (e.g. `mac.local`): use this machine's address on the
    // caller's connection instead.
    let local = conn.and_then(|c| c.local).map(|a| a.ip().to_canonical()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified()).map(|ip| match ip {
        IpAddr::V6(v6) => format!("[{v6}]"),
        v4 => v4.to_string(),
    });
    let framer = parts.headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(str::to_string);
    Caller { peer, host: header_host.or(local), framer }
}

/// Let a caller on another machine load `id`, and return its info with an
/// origin that machine can reach.
fn info_for_caller(id: &str, fallback: PreviewInfo, who: &Caller) -> PreviewInfo {
    let Some(entry) = appv3_preview::global().get(id) else { return fallback };
    if let Some(peer) = who.peer {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        entry.grant(peer, who.framer.as_deref(), now);
    }
    entry.info_for(who.host.as_deref())
}

fn preview_err(e: PreviewError) -> ApiError {
    match e {
        PreviewError::Invalid(m) => ApiError::unprocessable(m),
        PreviewError::Bind(m) => ApiError::internal(m),
    }
}

async fn open_preview(parts: Parts, raw: Bytes) -> ApiResult<Response> {
    let who = caller(&parts);
    let req: OpenRequest = serde_json::from_slice(&raw).map_err(|e| ApiError::unprocessable(format!("Invalid preview request: {e}")))?;
    let workspace = validate_workspace_or_422(&req.workspace, true)?;
    let (backend, path) = match (req.url.as_deref().filter(|s| !s.trim().is_empty()), req.path.as_deref().filter(|s| !s.trim().is_empty())) {
        (Some(url), None) => {
            let (target, path) = parse_url_target(url).map_err(|e| ApiError::unprocessable(e.to_string()))?;
            (Backend::Upstream(target), path)
        }
        (None, Some(rel)) => {
            let rel = resolve_workspace_file(Path::new(&workspace), rel).map_err(ApiError::unprocessable)?;
            (static_backend(Path::new(&workspace)), url_path(&rel))
        }
        _ => return Err(ApiError::unprocessable("Give either url or path.")),
    };
    let info = appv3_preview::global().ensure(&workspace, backend, req.preferred_port).await.map_err(preview_err)?;
    let info = info_for_caller(&info.id.clone(), info, &who);
    Ok(json(preview_json(&info, &path)))
}

async fn list_previews(parts: Parts, q: Qs) -> ApiResult<Response> {
    let who = caller(&parts);
    let workspace = match q.get("workspace") {
        Some(w) => Some(validate_workspace_or_422(w, false)?),
        None => None,
    };
    let previews: Vec<Value> = appv3_preview::global().list(workspace.as_deref()).iter().map(|e| preview_json(&info_for_caller(&e.id, e.info(), &who), "/")).collect();
    Ok(json(json!({ "previews": previews })))
}

async fn close_preview(AxPath(id): AxPath<String>) -> ApiResult<Response> {
    if appv3_preview::global().close(&id) {
        Ok(no_content())
    } else {
        Err(ApiError::not_found("Preview not found."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts(remote: &str, local: &str, host: &str, origin: Option<&str>) -> Parts {
        let mut b = Request::post("/api/preview").header(header::HOST, host);
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        let (mut p, _) = b.body(()).unwrap().into_parts();
        p.extensions.insert(ConnectInfo(ConnInfo { local: Some(local.parse().unwrap()), remote: Some(remote.parse().unwrap()) }));
        p
    }

    #[test]
    fn loopback_callers_get_no_grant_and_the_loopback_origin() {
        let who = caller(&parts("127.0.0.1:5000", "127.0.0.1:4082", "127.0.0.1:4082", Some("http://localhost:5173")));
        assert_eq!((who.peer, who.host, who.framer), (None, None, None));
    }

    #[test]
    fn other_machines_get_their_address_and_framer() {
        let who = caller(&parts("192.168.50.20:5000", "192.168.50.79:4082", "192.168.50.79:4082", Some("http://tauri.localhost")));
        assert_eq!(who.peer, Some("192.168.50.20".parse().unwrap()));
        assert_eq!(who.host.as_deref(), Some("192.168.50.79"));
        assert_eq!(who.framer.as_deref(), Some("http://tauri.localhost"));
    }

    #[test]
    fn a_name_host_falls_back_to_the_connection_address() {
        let who = caller(&parts("192.168.50.20:5000", "192.168.50.79:4082", "zachs-mac.local:4082", None));
        assert_eq!(who.host.as_deref(), Some("192.168.50.79"));
        let who = caller(&parts("[fd00::20]:5000", "[fd00::79]:4082", "zachs-mac.local:4082", None));
        assert_eq!(who.host.as_deref(), Some("[fd00::79]"));
    }

    #[tokio::test]
    async fn granting_lets_that_machine_load_the_preview() {
        let ws = tempfile::tempdir().unwrap();
        let (t, _) = parse_url_target("http://localhost:5199").unwrap();
        let info = appv3_preview::global().ensure(&ws.path().display().to_string(), Backend::Upstream(t), None).await.unwrap();
        let who = caller(&parts("192.168.50.20:5000", "192.168.50.79:4082", "192.168.50.79:4082", Some("http://192.168.50.79:5173")));
        let out = info_for_caller(&info.id.clone(), info.clone(), &who);
        assert_eq!(out.origin, format!("http://192.168.50.79:{}", info.port));
        let entry = appv3_preview::global().get(&info.id).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(entry.peer_allowed("192.168.50.20".parse().unwrap(), now));
        assert!(!entry.peer_allowed("192.168.50.21".parse().unwrap(), now));
        assert!(entry.frame_ancestors().ends_with(" http://192.168.50.79:5173"));
        appv3_preview::global().close(&info.id);
    }
}
