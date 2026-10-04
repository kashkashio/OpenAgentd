//! Preview registry: one listener per (workspace, backend).

use crate::agent::AgentChannel;
use crate::console::{ConsoleBuffer, ConsoleEntry};
use crate::target::Backend;
use appv3_tools::denied::DeniedPaths;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::watch;

/// Most listeners kept open at once; the least recently used one closes.
pub const MAX_PREVIEWS: usize = 8;
/// A preview with no open connection closes after this long.
pub const IDLE_SECS: u64 = 30 * 60;
/// How long another machine keeps access after its last authenticated
/// `/api/preview` call for the preview.
pub const GRANT_SECS: u64 = 12 * 60 * 60;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PreviewInfo {
    pub id: String,
    pub workspace: String,
    /// `url` for a dev server, `file` for a workspace directory.
    pub kind: &'static str,
    /// The proxied origin, or the served directory.
    pub target: String,
    pub port: u16,
    /// Where the iframe loads the preview from.
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewError {
    Invalid(String),
    Bind(String),
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreviewError::Invalid(m) | PreviewError::Bind(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PreviewError {}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct Entry {
    pub id: String,
    pub workspace: String,
    pub backend: Backend,
    pub port: u16,
    /// Denied-path rules for static previews.
    pub(crate) denied: Option<DeniedPaths>,
    last_active: AtomicU64,
    active: AtomicUsize,
    console: Mutex<ConsoleBuffer>,
    /// Commands from the agent to the page open in the Preview tab.
    pub agent: AgentChannel,
    stop: watch::Sender<bool>,
    /// Non-loopback machines allowed to load the preview, with the unix
    /// time their access ends. Granted by the authenticated API.
    grants: Mutex<HashMap<IpAddr, u64>>,
    /// App origins (beyond loopback and Tauri) allowed to frame the preview,
    /// e.g. the app served from another machine's address.
    framers: Mutex<Vec<String>>,
}

/// Marks a request or WebSocket as in flight; the preview is not idle while
/// one is alive.
pub struct ActiveGuard(Arc<Entry>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.touch();
    }
}

impl Entry {
    pub fn info(&self) -> PreviewInfo {
        self.info_for(None)
    }

    /// Info with `origin` on `host` (the address the caller reached the
    /// server at), or `127.0.0.1` for a loopback caller.
    pub fn info_for(&self, host: Option<&str>) -> PreviewInfo {
        let (kind, target) = match &self.backend {
            Backend::Upstream(t) => ("url", t.origin()),
            Backend::Static(root) => ("file", root.display().to_string()),
        };
        let host = host.filter(|h| !h.is_empty()).unwrap_or("127.0.0.1");
        PreviewInfo { id: self.id.clone(), workspace: self.workspace.clone(), kind, target, port: self.port, origin: format!("http://{host}:{}", self.port) }
    }

    /// Let `peer` (another machine) load this preview until `now + GRANT_SECS`,
    /// and let `framer` (the app origin it uses) embed it.
    pub fn grant(&self, peer: IpAddr, framer: Option<&str>, now: u64) {
        let peer = peer.to_canonical();
        if peer.is_loopback() {
            return;
        }
        self.grants.lock().unwrap_or_else(|e| e.into_inner()).insert(peer, now + GRANT_SECS);
        if let Some(origin) = framer.and_then(normalize_origin) {
            let mut f = self.framers.lock().unwrap_or_else(|e| e.into_inner());
            if !f.contains(&origin) {
                f.push(origin);
            }
        }
    }

    /// Loopback callers always; other machines only with a live grant.
    pub fn peer_allowed(&self, peer: IpAddr, now: u64) -> bool {
        let peer = peer.to_canonical();
        peer.is_loopback() || self.grants.lock().unwrap_or_else(|e| e.into_inner()).get(&peer).is_some_and(|until| *until > now)
    }

    /// The `frame-ancestors` directive for this preview's responses.
    pub fn frame_ancestors(&self) -> String {
        let f = self.framers.lock().unwrap_or_else(|e| e.into_inner());
        if f.is_empty() {
            crate::proxy::FRAME_ANCESTORS.to_string()
        } else {
            format!("{} {}", crate::proxy::FRAME_ANCESTORS, f.join(" "))
        }
    }

    fn touch(&self) {
        self.last_active.store(now_secs(), Ordering::SeqCst);
    }

    pub fn begin(self: &Arc<Self>) -> ActiveGuard {
        self.active.fetch_add(1, Ordering::SeqCst);
        self.touch();
        ActiveGuard(self.clone())
    }

    pub fn stop_rx(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    fn close(&self) {
        let _ = self.stop.send(true);
    }

    pub fn push_console(&self, entries: Vec<ConsoleEntry>) {
        let mut buf = self.console.lock().unwrap_or_else(|e| e.into_inner());
        for e in entries {
            buf.push(e);
        }
    }

    pub fn console(&self) -> Vec<ConsoleEntry> {
        self.console.lock().unwrap_or_else(|e| e.into_inner()).entries()
    }

    pub fn console_error_count(&self) -> usize {
        self.console.lock().unwrap_or_else(|e| e.into_inner()).error_count()
    }

    pub fn clear_console(&self) {
        self.console.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// `scheme://host[:port]` from an untrusted `Origin` header, or `None`. The
/// result goes into a CSP header, so nothing else may pass.
fn normalize_origin(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let origin = url.origin().ascii_serialization();
    (!origin.contains([' ', ';', ','])).then_some(origin)
}

#[derive(Default)]
pub struct Manager {
    entries: Mutex<Vec<Arc<Entry>>>,
    blocked_ports: Mutex<Vec<u16>>,
    reaper_started: AtomicBool,
    /// Listeners accept other machines (the server is LAN-exposed with an
    /// access key); otherwise they bind loopback only.
    lan: AtomicBool,
}

static GLOBAL: OnceLock<Manager> = OnceLock::new();

/// The process-wide preview registry.
pub fn global() -> &'static Manager {
    GLOBAL.get_or_init(|| {
        let m = Manager::default();
        // The configured API port is blocked even before the server binds.
        m.block_port(appv3_core::settings().api_port);
        m
    })
}

impl Manager {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<Entry>>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bind new listeners on all interfaces (`true`) or loopback only.
    /// Existing listeners keep their binding.
    pub fn set_lan(&self, lan: bool) {
        self.lan.store(lan, Ordering::SeqCst);
    }

    pub fn lan_enabled(&self) -> bool {
        self.lan.load(Ordering::SeqCst)
    }

    pub fn block_port(&self, port: u16) {
        let mut b = self.blocked_ports.lock().unwrap_or_else(|e| e.into_inner());
        if !b.contains(&port) {
            b.push(port);
        }
    }

    fn check_target(&self, backend: &Backend) -> Result<(), PreviewError> {
        let Backend::Upstream(t) = backend else { return Ok(()) };
        // The API and the other previews live on this machine's loopback;
        // an external site on the same port number is someone else's.
        if t.external {
            return Ok(());
        }
        let blocked = self.blocked_ports.lock().unwrap_or_else(|e| e.into_inner()).contains(&t.port);
        if blocked {
            return Err(PreviewError::Invalid(format!("Port {} is the OpenAgentd API and cannot be previewed.", t.port)));
        }
        if self.lock().iter().any(|e| e.port == t.port) {
            return Err(PreviewError::Invalid(format!("Port {} is another preview and cannot be previewed.", t.port)));
        }
        Ok(())
    }

    /// The preview for `backend` in `workspace`, starting its listener if
    /// needed. `preferred_port` keeps a target on the same origin (and so the
    /// same cookies and storage) across restarts when that port is free.
    pub async fn ensure(&'static self, workspace: &str, backend: Backend, preferred_port: Option<u16>) -> Result<PreviewInfo, PreviewError> {
        self.ensure_inner(workspace, backend, preferred_port, true).await
    }

    async fn ensure_inner(&'static self, workspace: &str, backend: Backend, preferred_port: Option<u16>, reaper: bool) -> Result<PreviewInfo, PreviewError> {
        self.check_target(&backend)?;
        if let Some(e) = self.find(workspace, &backend) {
            e.touch();
            return Ok(e.info());
        }
        let listener = bind(preferred_port, self.lan_enabled()).await?;
        let port = listener.local_addr().map_err(|e| PreviewError::Bind(e.to_string()))?.port();
        let denied = match &backend {
            Backend::Static(root) => Some(DeniedPaths::new(root, None)),
            Backend::Upstream(_) => None,
        };
        let (stop, stop_rx) = watch::channel(false);
        let entry = Arc::new(Entry {
            id: uuid::Uuid::new_v4().to_string(),
            workspace: workspace.to_string(),
            backend: backend.clone(),
            port,
            denied,
            last_active: AtomicU64::new(now_secs()),
            active: AtomicUsize::new(0),
            console: Mutex::new(ConsoleBuffer::default()),
            agent: AgentChannel::default(),
            stop,
            grants: Mutex::new(HashMap::new()),
            framers: Mutex::new(Vec::new()),
        });
        {
            let mut entries = self.lock();
            // Another request may have started the same preview meanwhile.
            if let Some(e) = entries.iter().find(|e| e.workspace == workspace && e.backend == backend) {
                return Ok(e.info());
            }
            if entries.len() >= MAX_PREVIEWS {
                let victim = entries.iter().enumerate().min_by_key(|(_, e)| (e.active.load(Ordering::SeqCst) > 0, e.last_active.load(Ordering::SeqCst))).map(|(i, _)| i);
                if let Some(i) = victim {
                    let old = entries.remove(i);
                    tracing::info!("preview_evicted id={} port={}", old.id, old.port);
                    old.close();
                }
            }
            entries.push(entry.clone());
        }
        tracing::info!("preview_started id={} port={} target={}", entry.id, port, entry.info().target);
        tokio::spawn(crate::proxy::serve(listener, entry.clone(), stop_rx));
        if reaper && !self.reaper_started.swap(true, Ordering::SeqCst) {
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    self.reap(now_secs(), IDLE_SECS);
                }
            });
        }
        Ok(entry.info())
    }

    fn find(&self, workspace: &str, backend: &Backend) -> Option<Arc<Entry>> {
        self.lock().iter().find(|e| e.workspace == workspace && &e.backend == backend).cloned()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Entry>> {
        self.lock().iter().find(|e| e.id == id).cloned()
    }

    /// Previews of `workspace` (every preview when `None`).
    pub fn list(&self, workspace: Option<&str>) -> Vec<Arc<Entry>> {
        self.lock().iter().filter(|e| workspace.is_none_or(|w| e.workspace == w)).cloned().collect()
    }

    pub fn close(&self, id: &str) -> bool {
        let mut entries = self.lock();
        let Some(i) = entries.iter().position(|e| e.id == id) else { return false };
        let e = entries.remove(i);
        tracing::info!("preview_closed id={} port={}", e.id, e.port);
        e.close();
        true
    }

    /// Close previews with no connection and no activity for `idle_secs`.
    fn reap(&self, now: u64, idle_secs: u64) -> Vec<String> {
        let mut entries = self.lock();
        let (idle, keep): (Vec<_>, Vec<_>) =
            entries.drain(..).partition(|e| e.active.load(Ordering::SeqCst) == 0 && now.saturating_sub(e.last_active.load(Ordering::SeqCst)) >= idle_secs);
        *entries = keep;
        idle.iter()
            .map(|e| {
                tracing::info!("preview_idle_closed id={} port={}", e.id, e.port);
                e.close();
                e.id.clone()
            })
            .collect()
    }

    pub fn shutdown(&self) {
        for e in self.lock().drain(..) {
            e.close();
        }
    }
}

async fn bind(preferred: Option<u16>, lan: bool) -> Result<tokio::net::TcpListener, PreviewError> {
    let addr = if lan { "0.0.0.0" } else { "127.0.0.1" };
    if let Some(p) = preferred.filter(|p| *p > 1024) {
        if let Ok(l) = tokio::net::TcpListener::bind((addr, p)).await {
            return Ok(l);
        }
    }
    tokio::net::TcpListener::bind((addr, 0)).await.map_err(|e| PreviewError::Bind(format!("Could not start the preview listener: {e}")))
}

/// A static backend for `root`, which must be an existing directory.
pub fn static_backend(root: &Path) -> Backend {
    Backend::Static(appv3_tools::denied::resolve(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::parse_url_target;

    fn leak() -> &'static Manager {
        Box::leak(Box::default())
    }

    fn upstream(raw: &str) -> Backend {
        Backend::Upstream(parse_url_target(raw).unwrap().0)
    }

    #[tokio::test]
    async fn reuses_the_preview_for_the_same_target() {
        let m = leak();
        let a = m.ensure_inner("/w", upstream("http://localhost:5173"), None, false).await.unwrap();
        let b = m.ensure_inner("/w", upstream("http://localhost:5173/other"), None, false).await.unwrap();
        let c = m.ensure_inner("/w", upstream("http://localhost:3000"), None, false).await.unwrap();
        let d = m.ensure_inner("/x", upstream("http://localhost:5173"), None, false).await.unwrap();
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, c.id);
        assert_ne!(a.id, d.id);
        assert_eq!(a.origin, format!("http://127.0.0.1:{}", a.port));
        assert_eq!(m.list(Some("/w")).len(), 2);
        m.shutdown();
    }

    #[tokio::test]
    async fn refuses_the_api_port_and_other_previews() {
        let m = leak();
        m.block_port(4082);
        assert!(m.ensure_inner("/w", upstream("http://127.0.0.1:4082"), None, false).await.is_err());
        let a = m.ensure_inner("/w", upstream("http://localhost:5173"), None, false).await.unwrap();
        let err = m.ensure_inner("/w", upstream(&format!("http://127.0.0.1:{}", a.port)), None, false).await.unwrap_err();
        assert!(err.to_string().contains("another preview"));
        m.shutdown();
    }

    #[tokio::test]
    async fn uses_a_free_preferred_port() {
        let m = leak();
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let a = m.ensure_inner("/w", upstream("http://localhost:5173"), Some(port), false).await.unwrap();
        assert_eq!(a.port, port);
        m.shutdown();
    }

    #[tokio::test]
    async fn evicts_the_least_recently_used_preview() {
        let m = leak();
        let first = m.ensure_inner("/w", upstream("http://localhost:9000"), None, false).await.unwrap();
        m.get(&first.id).unwrap().last_active.store(1, Ordering::SeqCst);
        for i in 1..MAX_PREVIEWS {
            m.ensure_inner("/w", upstream(&format!("http://localhost:{}", 9000 + i)), None, false).await.unwrap();
        }
        assert!(m.get(&first.id).is_some());
        m.ensure_inner("/w", upstream("http://localhost:9999"), None, false).await.unwrap();
        assert!(m.get(&first.id).is_none());
        assert_eq!(m.list(None).len(), MAX_PREVIEWS);
        m.shutdown();
    }

    #[tokio::test]
    async fn grants_other_machines_and_their_app_origins() {
        let m = leak();
        let a = m.ensure_inner("/w", upstream("http://localhost:5173"), None, false).await.unwrap();
        let e = m.get(&a.id).unwrap();
        let peer: IpAddr = "192.168.1.20".parse().unwrap();
        assert!(e.peer_allowed("127.0.0.1".parse().unwrap(), 0));
        assert!(e.peer_allowed("::ffff:127.0.0.1".parse().unwrap(), 0));
        assert!(!e.peer_allowed(peer, 0));
        e.grant(peer, Some("http://192.168.1.10:5173"), 100);
        assert!(e.peer_allowed(peer, 100));
        assert!(e.peer_allowed("::ffff:192.168.1.20".parse().unwrap(), 100));
        assert!(!e.peer_allowed(peer, 100 + GRANT_SECS));
        assert!(!e.peer_allowed("192.168.1.21".parse().unwrap(), 100));
        assert_eq!(e.frame_ancestors(), format!("{} http://192.168.1.10:5173", crate::proxy::FRAME_ANCESTORS));
        // A crafted Origin cannot inject CSP.
        e.grant(peer, Some("http://x.example; script-src *"), 100);
        e.grant(peer, Some("javascript:alert(1)"), 100);
        assert_eq!(e.frame_ancestors(), format!("{} http://192.168.1.10:5173", crate::proxy::FRAME_ANCESTORS));
        assert_eq!(e.info_for(Some("192.168.1.10")).origin, format!("http://192.168.1.10:{}", a.port));
        m.shutdown();
    }

    #[tokio::test]
    async fn external_sites_are_not_blocked_by_the_api_port() {
        let m = leak();
        m.block_port(443);
        assert!(m.ensure_inner("/w", upstream("https://develop.example.com"), None, false).await.is_ok());
        assert!(m.ensure_inner("/w", upstream("https://localhost:443"), None, false).await.is_err());
        m.shutdown();
    }

    #[tokio::test]
    async fn close_and_reap() {
        let m = leak();
        let a = m.ensure_inner("/w", upstream("http://localhost:5173"), None, false).await.unwrap();
        let b = m.ensure_inner("/w", upstream("http://localhost:3000"), None, false).await.unwrap();
        let busy = m.get(&b.id).unwrap();
        let _guard = busy.begin();
        assert_eq!(m.reap(now_secs() + IDLE_SECS + 1, IDLE_SECS), vec![a.id.clone()]);
        assert!(m.get(&b.id).is_some());
        assert!(m.close(&b.id));
        assert!(!m.close(&b.id));
    }
}
