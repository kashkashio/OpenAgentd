//! Interactive PTY sessions for the user-facing terminal — port of
//! `app/services/terminal_service.py` on top of `portable-pty`.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Hard cap on simultaneously open PTY sessions (`MAX_SESSIONS`).
pub const MAX_SESSIONS: usize = 8;
/// Idle sessions are reaped after this long (`IDLE_TIMEOUT_SECONDS`), unless
/// a command is still running in them ([`TerminalSession::busy`]).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const READ_CHUNK: usize = 65536;
const READ_QUEUE: usize = 64;
/// Outer-terminal identity vars that must not leak into the spawned shell.
const IDENTITY_LEAK_KEYS: [&str; 2] = ["TERM_SESSION_ID", "ITERM_SESSION_ID"];

pub struct TerminalSession {
    pub session_id: String,
    pub pid: Option<u32>,
    pub workspace: String,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    rx: tokio::sync::Mutex<mpsc::Receiver<Option<Vec<u8>>>>,
    last_activity: Arc<Mutex<Instant>>,
    closed: std::sync::atomic::AtomicBool,
    eof: Arc<std::sync::atomic::AtomicBool>,
}

fn sessions() -> &'static Mutex<HashMap<String, Arc<TerminalSession>>> {
    static S: OnceLock<Mutex<HashMap<String, Arc<TerminalSession>>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

pub fn get_session(id: &str) -> Option<Arc<TerminalSession>> {
    sessions().lock().unwrap().get(id).cloned()
}

pub fn session_count() -> usize {
    sessions().lock().unwrap().len()
}

#[cfg(unix)]
fn killpg(pid: u32, sig: nix::sys::signal::Signal) {
    use nix::unistd::{getpgid, Pid};
    if let Ok(pg) = getpgid(Some(Pid::from_raw(pid as i32))) {
        let _ = nix::sys::signal::killpg(pg, sig);
    }
}

impl TerminalSession {
    fn touch(&self) {
        *self.last_activity.lock().unwrap() = Instant::now();
    }

    pub fn idle_for(&self) -> Duration {
        self.last_activity.lock().unwrap().elapsed()
    }

    /// Next output chunk; `None` signals EOF.
    pub async fn read(&self) -> Option<Vec<u8>> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some(Some(c)) => Some(c),
            _ => None,
        }
    }

    pub async fn write(&self, data: Vec<u8>) -> std::io::Result<()> {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) || self.eof.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        self.touch();
        let w = self.writer.clone();
        tokio::task::spawn_blocking(move || {
            let mut g = w.lock().unwrap();
            match g.as_mut() {
                Some(w) => w.write_all(&data).and_then(|_| w.flush()),
                None => Ok(()),
            }
        })
        .await
        .unwrap_or(Ok(()))
    }

    /// TIOCSWINSZ + SIGWINCH, clamped like v2.
    pub fn resize(&self, rows: i64, cols: i64) {
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let rows = rows.clamp(1, 1000) as u16;
        let cols = cols.clamp(1, 4000) as u16;
        if let Some(m) = self.master.lock().unwrap().as_ref() {
            let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            killpg(pid, nix::sys::signal::Signal::SIGWINCH);
        }
    }

    fn process_alive(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    pub fn alive(&self) -> bool {
        !self.closed.load(std::sync::atomic::Ordering::SeqCst) && !self.eof.load(std::sync::atomic::Ordering::SeqCst) && self.process_alive()
    }

    /// True while a command runs in the shell: the PTY's foreground process
    /// group is no longer the shell's own (job control hands the terminal to
    /// each command it runs). Windows has no such signal and reads false.
    pub fn busy(&self) -> bool {
        #[cfg(unix)]
        {
            let Some(pid) = self.pid else { return false };
            let foreground = self.master.lock().unwrap().as_ref().and_then(|m| m.process_group_leader());
            matches!(foreground, Some(pg) if i64::from(pg) != i64::from(pid))
        }
        #[cfg(not(unix))]
        false
    }

    /// SIGHUP the process group, grace period, then SIGKILL; release the PTY.
    pub async fn close(&self) {
        if self.closed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        sessions().lock().unwrap().remove(&self.session_id);
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            killpg(pid, nix::sys::signal::Signal::SIGHUP);
        }
        let mut exited = false;
        for _ in 0..10 {
            if !self.process_alive() {
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !exited {
            #[cfg(unix)]
            if let Some(pid) = self.pid {
                killpg(pid, nix::sys::signal::Signal::SIGKILL);
            }
            let _ = self.child.lock().unwrap().kill();
            for _ in 0..40 {
                if !self.process_alive() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        self.writer.lock().unwrap().take();
        self.master.lock().unwrap().take();
        tracing::info!("terminal_session_closed session_id={} pid={:?}", self.session_id, self.pid);
    }
}

/// `create_session` — spawn the user's shell in a PTY rooted at *workspace*.
/// `Err(message)` mirrors v2's RuntimeError / OSError text.
/// Windows uses ConPTY (v2 refuses terminals there).
pub fn create_session(workspace: &str, rows: i64, cols: i64) -> Result<Arc<TerminalSession>, String> {
    if session_count() >= MAX_SESSIONS {
        return Err(format!("Too many open terminal sessions (max {MAX_SESSIONS}). Close an existing terminal first."));
    }
    let shell = appv3_tools::shell::acceptable();
    let name = appv3_tools::shell::shell_name_of(&shell);
    let mut cmd = CommandBuilder::new(&shell);
    match name.as_str() {
        "zsh" | "bash" => cmd.arg("-il"),
        "pwsh" | "powershell" => cmd.arg("-NoLogo"),
        "cmd" => {}
        _ => cmd.arg("-i"),
    }
    for k in appv3_tools::shell::LEAK_KEYS.iter().chain(IDENTITY_LEAK_KEYS.iter()) {
        cmd.env_remove(k);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.cwd(workspace);
    let size = PtySize { rows: rows.clamp(1, 1000) as u16, cols: cols.clamp(1, 4000) as u16, pixel_width: 0, pixel_height: 0 };
    let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<Option<Vec<u8>>>(READ_QUEUE);
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let eof = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (la, eo) = (last_activity.clone(), eof.clone());
    std::thread::Builder::new()
        .name("terminal-reader".into())
        .spawn(move || {
            let mut buf = vec![0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        *la.lock().unwrap() = Instant::now();
                        if tx.blocking_send(Some(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
            eo.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = tx.blocking_send(None);
        })
        .map_err(|e| e.to_string())?;
    let session = Arc::new(TerminalSession {
        session_id: uuid::Uuid::new_v4().simple().to_string(),
        pid: child.process_id(),
        workspace: workspace.to_string(),
        master: Mutex::new(Some(pair.master)),
        writer: Arc::new(Mutex::new(Some(writer))),
        child: Mutex::new(child),
        rx: tokio::sync::Mutex::new(rx),
        last_activity,
        closed: Default::default(),
        eof,
    });
    session.resize(rows, cols);
    sessions().lock().unwrap().insert(session.session_id.clone(), session.clone());
    ensure_reaper();
    tracing::info!("terminal_session_created session_id={} pid={:?} shell={} workspace={}", session.session_id, session.pid, name, workspace);
    Ok(session)
}

fn ensure_reaper() {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        return;
    };
    handle.spawn(async {
        let tick = (IDLE_TIMEOUT / 10).clamp(Duration::from_millis(50), Duration::from_secs(30));
        while session_count() > 0 {
            tokio::time::sleep(tick).await;
            let all: Vec<Arc<TerminalSession>> = sessions().lock().unwrap().values().cloned().collect();
            for s in all {
                // A running command keeps its session, however quiet it is.
                if !s.alive() || (s.idle_for() > IDLE_TIMEOUT && !s.busy()) {
                    tracing::info!("terminal_session_reaped session_id={} idle_s={:.0}", s.session_id, s.idle_for().as_secs_f64());
                    s.close().await;
                }
            }
        }
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// `close_all` (shutdown).
pub async fn close_all() {
    let all: Vec<Arc<TerminalSession>> = sessions().lock().unwrap().values().cloned().collect();
    for s in all {
        s.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_roundtrip() {
        if appv3_core::platform::under_wine() {
            // Wine's ConPTY starts the shell (screen clear + title arrive) but
            // never delivers its prompt or reads input. CI covers real Windows.
            eprintln!("echo_roundtrip skipped: Wine ConPTY does not pass shell I/O");
            return;
        }
        // Pin a plain shell: the developer's login shell rc files (e.g. an
        // oh-my-zsh update prompt) can swallow the scripted input. This is
        // the only test in the binary, so the process-wide env is safe.
        std::env::set_var("SHELL", "/bin/sh");
        // The server's own access token must not reach the user's terminal.
        std::env::set_var("OPENAGENTD_DESKTOP_TOKEN", "oad-leak-canary");
        let d = std::env::temp_dir();
        let s = create_session(&d.display().to_string(), 24, 80).unwrap();
        // The output must differ from the echoed input, so compute the "42".
        let script: &[u8] = match appv3_tools::shell::shell_name_of(&appv3_tools::shell::acceptable()).as_str() {
            "pwsh" | "powershell" => b"'oad_term_' + (40+2) + '[' + $env:OPENAGENTD_DESKTOP_TOKEN + ']'\r\nexit\r\n",
            "cmd" => b"echo oad_term_4^2[%OPENAGENTD_DESKTOP_TOKEN%]\r\nexit\r\n",
            _ => b"echo \"oad_term_$((40+2))[${OPENAGENTD_DESKTOP_TOKEN}]\"\nexit\n",
        };
        s.write(script.to_vec()).await.unwrap();
        let mut out = String::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(20), s.read()).await {
                Ok(Some(c)) => out.push_str(&String::from_utf8_lossy(&c)),
                _ => break,
            }
            if out.contains("oad_term_42") {
                break;
            }
        }
        assert!(out.contains("oad_term_42"), "{out:?}");
        assert!(!out.contains("oad-leak-canary"), "desktop token leaked into the terminal: {out:?}");
        // The prompt is idle: no command in the foreground.
        assert!(!s.busy(), "idle shell read as busy");
        s.close().await;
        assert!(get_session(&s.session_id).is_none());

        // A running command makes the session busy; it reads idle again once
        // the command finishes. Windows has no foreground process group.
        #[cfg(unix)]
        {
            let s = create_session(&d.display().to_string(), 24, 80).unwrap();
            let output = drain(&s);
            s.write(b"sleep 1\n".to_vec()).await.unwrap();
            assert!(wait_for(|| s.busy()).await, "running command not reported busy");
            assert!(wait_for(|| !s.busy()).await, "finished command still reported busy");
            s.close().await;
            output.abort();
        }
    }

    /// Keep reading output so the shell never blocks on a full PTY.
    #[cfg(unix)]
    fn drain(s: &Arc<TerminalSession>) -> tokio::task::JoinHandle<()> {
        let s = s.clone();
        tokio::spawn(async move { while s.read().await.is_some() {} })
    }

    #[cfg(unix)]
    async fn wait_for(cond: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }
}
