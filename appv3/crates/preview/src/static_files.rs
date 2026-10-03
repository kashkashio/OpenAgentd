//! Serve a workspace directory for previews of plain HTML files.
//!
//! Paths stay inside the root, and the same denied-path rules as the file
//! tools apply, so `.git`, `.env*`, keys, and the app's data dirs are never
//! served.

use crate::inject::inject;
use crate::manager::Entry;
use crate::proxy::plain;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use std::path::{Path, PathBuf};

/// Largest file served; previews are for pages, not archives.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Percent-encode a workspace-relative path for use as a URL path.
pub fn url_path(rel: &str) -> String {
    const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
    let encoded: Vec<String> = rel.split('/').filter(|s| !s.is_empty()).map(|s| percent_encoding::utf8_percent_encode(s, SEGMENT).to_string()).collect();
    format!("/{}", encoded.join("/"))
}

/// A workspace file to preview: relative, inside the root, not denied.
pub fn resolve_workspace_file(root: &Path, rel: &str) -> Result<String, String> {
    let rel = rel.trim().trim_start_matches("./").replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') || (rel.len() >= 2 && rel.as_bytes()[1] == b':') {
        return Err("path must be relative to the workspace.".into());
    }
    if Path::new(&rel).components().any(|c| matches!(c, std::path::Component::ParentDir)) || appv3_core::security::is_denied_path(Path::new(&rel)) {
        return Err(format!("'{rel}' cannot be previewed."));
    }
    let root = appv3_tools::denied::resolve(root);
    let resolved = appv3_tools::denied::resolve(&root.join(&rel));
    if !resolved.starts_with(&root) {
        return Err(format!("'{rel}' is outside the workspace."));
    }
    if !resolved.is_file() {
        return Err(format!("'{rel}' is not a file in the workspace."));
    }
    Ok(rel)
}

#[derive(Debug, PartialEq)]
pub(crate) enum Resolved {
    File(PathBuf),
    /// A directory requested without a trailing slash.
    RedirectToSlash,
    Denied,
    NotFound,
}

/// Map a URL path onto a file under `root` (already resolved).
pub(crate) fn resolve_path(root: &Path, url_path: &str, denied: Option<&appv3_tools::denied::DeniedPaths>) -> Resolved {
    let decoded = percent_encoding::percent_decode_str(url_path).decode_utf8_lossy().to_string();
    if decoded.contains('\0') || decoded.contains('\\') {
        return Resolved::NotFound;
    }
    let rel = decoded.trim_start_matches('/');
    let rel_path = Path::new(rel);
    if rel_path.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_) | std::path::Component::RootDir)) {
        return Resolved::NotFound;
    }
    if appv3_core::security::is_denied_path(rel_path) {
        return Resolved::Denied;
    }
    let resolved = appv3_tools::denied::resolve(&root.join(rel_path));
    if !resolved.starts_with(root) {
        return Resolved::Denied;
    }
    if let Ok(inner) = resolved.strip_prefix(root) {
        if appv3_core::security::is_denied_path(inner) {
            return Resolved::Denied;
        }
    }
    if denied.is_some_and(|d| d.is_denied_read_path(&resolved)) {
        return Resolved::Denied;
    }
    if resolved.is_dir() {
        if !decoded.is_empty() && !decoded.ends_with('/') {
            return Resolved::RedirectToSlash;
        }
        let index = resolved.join("index.html");
        return if index.is_file() { Resolved::File(index) } else { Resolved::NotFound };
    }
    if resolved.is_file() {
        Resolved::File(resolved)
    } else {
        Resolved::NotFound
    }
}

pub(crate) async fn serve(entry: &Entry, root: &Path, req: Request) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed.");
    }
    let head = req.method() == Method::HEAD;
    let url_path = req.uri().path().to_string();
    let file = match resolve_path(root, &url_path, entry.denied.as_ref()) {
        Resolved::File(f) => f,
        Resolved::RedirectToSlash => {
            let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
            return (StatusCode::FOUND, [(header::LOCATION, format!("{url_path}/{query}")), (header::CACHE_CONTROL, "no-store".to_string())]).into_response();
        }
        Resolved::Denied => return plain(StatusCode::FORBIDDEN, "This path is not served by the preview."),
        Resolved::NotFound => return plain(StatusCode::NOT_FOUND, "Not found."),
    };
    let size = tokio::fs::metadata(&file).await.map(|m| m.len()).unwrap_or(0);
    if size > MAX_FILE_BYTES {
        return plain(StatusCode::PAYLOAD_TOO_LARGE, "File too large to preview.");
    }
    let name = file.to_string_lossy().to_string();
    let media = appv3_core::mimetypes::guess_type(&name).unwrap_or_else(|| "application/octet-stream".to_string());
    let html = media == "text/html";
    let content_type = if media.starts_with("text/") || media == "application/javascript" { format!("{media}; charset=utf-8") } else { media };
    if !html {
        let range = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok());
        return file_response(&file, size, content_type, head, range).await;
    }
    let body = match tokio::fs::read(&file).await {
        Ok(b) => b,
        Err(e) => return plain(StatusCode::NOT_FOUND, &format!("Could not read file: {e}")),
    };
    let body = inject(&body);
    let len = body.len();
    let mut resp = Response::new(if head { Body::empty() } else { Body::from(body) });
    let h = resp.headers_mut();
    if let Ok(v) = content_type.parse() {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CONTENT_LENGTH, len.into());
    h.insert(header::CACHE_CONTROL, "no-store".parse().expect("static header"));
    crate::proxy::add_frame_ancestors(h);
    resp
}

/// One `bytes=` range as `[start, end)` within `size`. `Ok(None)` serves the
/// whole file: no header, several ranges, other units or a malformed spec
/// (RFC 9110 lets a server ignore Range). `Err(())`: it starts past the end.
fn single_range(raw: Option<&str>, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = raw.and_then(|r| r.trim().strip_prefix("bytes=")) else { return Ok(None) };
    let Some((s, e)) = spec.split_once('-').filter(|_| size > 0 && !spec.contains(',')) else { return Ok(None) };
    let (s, e) = (s.trim(), e.trim());
    if s.is_empty() {
        return Ok(e.parse::<u64>().ok().filter(|n| *n > 0).map(|n| (size.saturating_sub(n), size)));
    }
    let Ok(start) = s.parse::<u64>() else { return Ok(None) };
    let end = match e {
        "" => size,
        e => match e.parse::<u64>() {
            Ok(last) if last >= start => (last + 1).min(size),
            _ => return Ok(None),
        },
    };
    if start >= size {
        return Err(());
    }
    Ok(Some((start, end)))
}

/// Streams a non-HTML file, or the one byte range asked for: browsers open
/// and seek media with `Range`, and a whole-file read would buffer up to
/// [`MAX_FILE_BYTES`] per request.
async fn file_response(file: &Path, size: u64, content_type: String, head: bool, range: Option<&str>) -> Response {
    let (status, start, end) = match single_range(range, size) {
        Ok(None) => (StatusCode::OK, 0, size),
        Ok(Some((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e),
        Err(()) => {
            let mut r = plain(StatusCode::RANGE_NOT_SATISFIABLE, "Range not satisfiable.");
            if let Ok(v) = format!("bytes */{size}").parse() {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return r;
        }
    };
    let body = if head {
        Body::empty()
    } else {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let opened = async {
            let mut f = tokio::fs::File::open(file).await?;
            f.seek(std::io::SeekFrom::Start(start)).await?;
            Ok::<_, std::io::Error>(f.take(end - start))
        };
        match opened.await {
            Ok(f) => Body::from_stream(tokio_util::io::ReaderStream::with_capacity(f, 64 * 1024)),
            Err(e) => return plain(StatusCode::NOT_FOUND, &format!("Could not read file: {e}")),
        }
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    if let Ok(v) = content_type.parse() {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CONTENT_LENGTH, (end - start).into());
    h.insert(header::ACCEPT_RANGES, header::HeaderValue::from_static("bytes"));
    if status == StatusCode::PARTIAL_CONTENT {
        if let Ok(v) = format!("bytes {start}-{}/{size}", end - 1).parse() {
            h.insert(header::CONTENT_RANGE, v);
        }
    }
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    crate::proxy::add_frame_ancestors(h);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = appv3_tools::denied::resolve(dir.path());
        std::fs::create_dir_all(root.join("site/css")).unwrap();
        std::fs::write(root.join("site/index.html"), "<html></html>").unwrap();
        std::fs::write(root.join("site/css/a.css"), "a{}").unwrap();
        std::fs::write(root.join("page.html"), "<p>").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        (dir, root)
    }

    #[test]
    fn serves_files_and_directory_indexes() {
        let (_d, r) = root();
        assert_eq!(resolve_path(&r, "/page.html", None), Resolved::File(r.join("page.html")));
        assert_eq!(resolve_path(&r, "/site/", None), Resolved::File(r.join("site/index.html")));
        assert_eq!(resolve_path(&r, "/site", None), Resolved::RedirectToSlash);
        assert_eq!(resolve_path(&r, "/site/css/a.css", None), Resolved::File(r.join("site/css/a.css")));
        assert_eq!(resolve_path(&r, "/site/css/", None), Resolved::NotFound);
        assert_eq!(resolve_path(&r, "/missing.html", None), Resolved::NotFound);
    }

    #[test]
    fn refuses_traversal_and_denied_paths() {
        let (_d, r) = root();
        assert_eq!(resolve_path(&r, "/../etc/passwd", None), Resolved::NotFound);
        assert_eq!(resolve_path(&r, "/%2e%2e/etc/passwd", None), Resolved::NotFound);
        assert_eq!(resolve_path(&r, "/site/%2E%2E/%2E%2E/x", None), Resolved::NotFound);
        assert_eq!(resolve_path(&r, "/.git/config", None), Resolved::Denied);
        assert_eq!(resolve_path(&r, "/.env", None), Resolved::Denied);
        assert_eq!(resolve_path(&r, "/a%00.html", None), Resolved::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_that_leave_the_root() {
        let (_d, r) = root();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), r.join("link.txt")).unwrap();
        assert_eq!(resolve_path(&r, "/link.txt", None), Resolved::Denied);
    }

    #[test]
    fn encodes_url_paths() {
        assert_eq!(url_path("designs/landing page.html"), "/designs/landing%20page.html");
        assert_eq!(url_path("a/ü#.html"), "/a/%C3%BC%23.html");
    }

    #[test]
    fn parses_one_byte_range() {
        assert_eq!(single_range(None, 100), Ok(None));
        assert_eq!(single_range(Some("bytes=0-"), 100), Ok(Some((0, 100))));
        assert_eq!(single_range(Some("bytes=10-19"), 100), Ok(Some((10, 20))));
        assert_eq!(single_range(Some("bytes=90-500"), 100), Ok(Some((90, 100))));
        assert_eq!(single_range(Some("bytes=-10"), 100), Ok(Some((90, 100))));
        assert_eq!(single_range(Some("bytes=-500"), 100), Ok(Some((0, 100))));
        assert_eq!(single_range(Some("bytes=100-"), 100), Err(()));
        // Ignored (whole file): several ranges, other units, malformed specs.
        for raw in ["bytes=0-1,5-6", "items=0-1", "bytes=5-2", "bytes=x-", "bytes=-0", "bytes=-"] {
            assert_eq!(single_range(Some(raw), 100), Ok(None), "{raw}");
        }
    }

    async fn body_of(r: Response) -> Vec<u8> {
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    #[tokio::test]
    async fn streams_files_and_honours_a_single_range() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("clip.mp4");
        let data: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
        std::fs::write(&f, &data).unwrap();
        let size = data.len() as u64;

        let full = file_response(&f, size, "video/mp4".into(), false, None).await;
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(full.headers()[header::CONTENT_LENGTH], "300000");
        assert_eq!(body_of(full).await, data);

        let part = file_response(&f, size, "video/mp4".into(), false, Some("bytes=1000-1999")).await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 1000-1999/300000");
        assert_eq!(part.headers()[header::CONTENT_LENGTH], "1000");
        assert_eq!(body_of(part).await, &data[1000..2000]);

        let past = file_response(&f, size, "video/mp4".into(), false, Some("bytes=300000-")).await;
        assert_eq!(past.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(past.headers()[header::CONTENT_RANGE], "bytes */300000");

        let head = file_response(&f, size, "video/mp4".into(), true, Some("bytes=0-")).await;
        assert_eq!(head.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "300000");
        assert!(body_of(head).await.is_empty());
    }

    #[test]
    fn resolves_only_workspace_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "x").unwrap();
        std::fs::write(dir.path().join(".env"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert_eq!(resolve_workspace_file(dir.path(), "./index.html").unwrap(), "index.html");
        for bad in ["", "/etc/passwd", "../x.html", ".env", "sub", "missing.html", "C:/x.html"] {
            assert!(resolve_workspace_file(dir.path(), bad).is_err(), "{bad}");
        }
    }
}
