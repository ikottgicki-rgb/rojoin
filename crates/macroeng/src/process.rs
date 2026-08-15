//! Suspending and resuming the game process.
//!
//! This is the "freeze" technique: stop the Roblox process for a moment and
//! let it resume. It is process control, not memory access — the same thing
//! `kill -STOP` does from a shell.
//!
//! **Safety is the whole design here.** A process left suspended is a hung
//! game, so every freeze is time-boxed, the engine always resumes on stop, and
//! `resume_all` runs on shutdown. Nothing suspends indefinitely.

use std::sync::Mutex;

use crate::{Error, Result};

/// Everything currently suspended by us, so it can always be released.
static SUSPENDED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Find the running game process.
///
/// On Linux, Sober runs the engine under bwrap, so the useful match is on the
/// executable name rather than the full command line.
#[cfg(unix)]
pub fn find_game_pid() -> Option<i32> {
    const NEEDLES: &[&str] = &["sober", "RobloxPlayer"];

    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid: i32 = name.to_str()?.parse().ok()?;

        // `comm` is the executable name, which survives the bwrap wrapping
        // that makes the cmdline useless here.
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        let comm = comm.trim();

        if NEEDLES.iter().any(|n| comm.eq_ignore_ascii_case(n)) {
            return Some(pid);
        }
    }
    None
}

#[cfg(not(unix))]
pub fn find_game_pid() -> Option<i32> {
    // Windows would use CreateToolhelp32Snapshot plus NtSuspendProcess; not
    // implemented, and saying so beats pretending to freeze nothing.
    None
}

#[cfg(unix)]
fn signal(pid: i32, sig: i32) -> Result<()> {
    // SAFETY: kill() with a pid we read from /proc; a dead pid returns ESRCH
    // rather than doing anything dangerous.
    let rc = unsafe { libc::kill(pid, sig) };
    if rc != 0 {
        return Err(Error::Input(format!(
            "could not signal {pid}: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub fn suspend(pid: i32) -> Result<()> {
    signal(pid, libc::SIGSTOP)?;
    if let Ok(mut s) = SUSPENDED.lock() {
        if !s.contains(&pid) {
            s.push(pid);
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn resume(pid: i32) -> Result<()> {
    let r = signal(pid, libc::SIGCONT);
    if let Ok(mut s) = SUSPENDED.lock() {
        s.retain(|p| *p != pid);
    }
    r
}

#[cfg(not(unix))]
pub fn suspend(_pid: i32) -> Result<()> {
    Err(Error::NoBackend("process freeze is not implemented on this platform".into()))
}

#[cfg(not(unix))]
pub fn resume(_pid: i32) -> Result<()> {
    Ok(())
}

/// Release everything we suspended. Called on engine stop and at shutdown, so
/// a crash mid-freeze cannot leave the game hung.
pub fn resume_all() {
    let pids: Vec<i32> = SUSPENDED.lock().map(|s| s.clone()).unwrap_or_default();
    for pid in pids {
        let _ = resume(pid);
    }
}

pub fn anything_suspended() -> bool {
    SUSPENDED.lock().map(|s| !s.is_empty()).unwrap_or(false)
}

/// Longest a single freeze may last. A freeze is a brief interruption; a
/// multi-second one is a hung game, so the value is clamped rather than
/// trusted.
pub const MAX_FREEZE_MS: u64 = 2_000;

pub fn clamp_freeze(ms: u64) -> u64 {
    ms.clamp(1, MAX_FREEZE_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_duration_is_clamped_to_something_survivable() {
        assert_eq!(clamp_freeze(0), 1);
        assert_eq!(clamp_freeze(250), 250);
        // A macro asking for a minute-long freeze is a hung game, not a freeze.
        assert_eq!(clamp_freeze(60_000), MAX_FREEZE_MS);
    }

    #[test]
    fn nothing_is_suspended_to_begin_with() {
        assert!(!anything_suspended());
    }

    #[test]
    fn resume_all_is_safe_when_nothing_is_suspended() {
        resume_all();
        assert!(!anything_suspended());
    }
}
