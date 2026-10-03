//! The background server's PID file and log, under the active state dir.

use std::path::PathBuf;

pub fn pid_file() -> PathBuf {
    appv3_core::settings().state_dir.join("openagentd.pid")
}

pub fn server_log() -> PathBuf {
    appv3_core::settings().state_dir.join("logs").join("app").join("app.log")
}

pub fn write_pids(pids: &[u32]) -> std::io::Result<()> {
    let f = pid_file();
    if let Some(p) = f.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(f, pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("\n"))
}

/// PIDs in the file; any unparsable line discards the whole file.
pub fn read_pids() -> Vec<i32> {
    let Ok(text) = std::fs::read_to_string(pid_file()) else { return vec![] };
    let pids: Option<Vec<i32>> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| l.parse().ok()).collect();
    pids.unwrap_or_default()
}

#[cfg(unix)]
pub fn pid_alive(pid: i32) -> bool {
    use nix::errno::Errno;
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// Windows: the process exists and has not exited (`STILL_ACTIVE`).
#[cfg(windows)]
pub fn pid_alive(pid: i32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if pid <= 0 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
        if h.is_null() {
            // Access denied still means the process exists (like EPERM on Unix).
            return std::io::Error::last_os_error().raw_os_error() == Some(5);
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

pub fn find_pids() -> Vec<i32> {
    read_pids().into_iter().filter(|&p| pid_alive(p)).collect()
}

pub fn clear_pids() {
    let _ = std::fs::remove_file(pid_file());
}

/// Blocks until `pid` exits, without polling. `Some(())` once it has exited
/// or if it was already gone; `None` when the kernel can't wait on it (old
/// kernel, no permission), so the caller falls back to `pid_alive` polling.
/// Unlike `kill(pid, 0)`, this can't be fooled by a reused pid.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
pub fn wait_pid_exit(pid: i32) -> Option<()> {
    use std::io::{Error, ErrorKind};
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return None;
        }
        let mut ev: libc::kevent = std::mem::zeroed();
        ev.ident = pid as libc::uintptr_t;
        ev.filter = libc::EVFILT_PROC;
        ev.flags = libc::EV_ADD | libc::EV_ONESHOT;
        ev.fflags = libc::NOTE_EXIT;
        // No event list here, so a failed registration reports through errno.
        if libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) < 0 {
            let gone = Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            libc::close(kq);
            return gone.then_some(());
        }
        let mut out: libc::kevent = std::mem::zeroed();
        let res = loop {
            let n = libc::kevent(kq, std::ptr::null(), 0, &mut out, 1, std::ptr::null());
            if n > 0 {
                break Some(());
            }
            if n < 0 && Error::last_os_error().kind() != ErrorKind::Interrupted {
                break None;
            }
        };
        libc::close(kq);
        res
    }
}

#[cfg(target_os = "linux")]
pub fn wait_pid_exit(pid: i32) -> Option<()> {
    use std::io::{Error, ErrorKind};
    unsafe {
        // pidfd_open (Linux 5.3+): readable once the process exits.
        let fd = libc::syscall(libc::SYS_pidfd_open, pid, 0) as libc::c_int;
        if fd < 0 {
            return (Error::last_os_error().raw_os_error() == Some(libc::ESRCH)).then_some(());
        }
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let res = loop {
            let n = libc::poll(&mut p, 1, -1);
            if n > 0 {
                break Some(());
            }
            if n < 0 && Error::last_os_error().kind() != ErrorKind::Interrupted {
                break None;
            }
        };
        libc::close(fd);
        res
    }
}

#[cfg(windows)]
pub fn wait_pid_exit(pid: i32) -> Option<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE};
    if pid <= 0 {
        return Some(());
    }
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid as u32);
        if h.is_null() {
            // ERROR_INVALID_PARAMETER: no such process. Anything else (access
            // denied) means it may exist, so let the caller poll.
            return (std::io::Error::last_os_error().raw_os_error() == Some(87)).then_some(());
        }
        let r = WaitForSingleObject(h, INFINITE);
        CloseHandle(h);
        (r == WAIT_OBJECT_0).then_some(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "linux", windows)))]
pub fn wait_pid_exit(_pid: i32) -> Option<()> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn wait_pid_exit_returns_once_the_process_exits() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || tx.send(wait_pid_exit(pid)).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "returned while the process was alive");
        child.kill().unwrap();
        // Not reaped yet: a zombie still counts as alive for `kill(pid, 0)`,
        // but it has exited, which is what the parent watch needs to see.
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(()));
        child.wait().unwrap();
    }

    #[test]
    fn wait_pid_exit_returns_at_once_for_a_gone_process() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert_eq!(wait_pid_exit(pid), Some(()));
    }
}
