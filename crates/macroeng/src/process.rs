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
static SUSPENDED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Process names that mean "the game".
///
/// Sober runs the engine under bwrap, so several processes share the name and
/// suspending only one leaves the rest running — hence freezing all matches.
const NEEDLES: &[&str] = &["sober", "robloxplayerbeta", "robloxplayer", "windowsplayer"];

fn is_game(comm: &str) -> bool {
    let c = comm.trim().trim_end_matches(".exe").to_ascii_lowercase();
    NEEDLES.iter().any(|n| c == *n)
}

#[cfg(unix)]
pub fn find_game_pids() -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    let mut pids = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Ok(pid) = name.parse::<u32>() else { continue };

        let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) else { continue };
        if is_game(&comm) {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(unix)]
fn signal(pid: u32, sig: i32) -> Result<()> {
    let rc = unsafe { libc::kill(pid as i32, sig) };
    if rc != 0 {
        return Err(Error::Input(format!(
            "could not signal {pid}: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn suspend_one(pid: u32) -> Result<()> {
    signal(pid, libc::SIGSTOP)
}

#[cfg(unix)]
fn resume_one(pid: u32) -> Result<()> {
    signal(pid, libc::SIGCONT)
}

#[cfg(windows)]
pub fn find_game_pids() -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;

    let mut pids = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return pids;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                if is_game(&name) {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    pids
}

/// `NtSuspendProcess`/`NtResumeProcess` are undocumented but stable, and are
/// how every process-freezer on Windows does this. Resolved at runtime so the
/// binary does not need an import for them.
#[cfg(windows)]
unsafe fn ntdll_call(name: &[u8], pid: u32) -> Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};

    let module = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
    if module.is_null() {
        return Err(Error::Input("could not load ntdll".into()));
    }

    let Some(proc_addr) = GetProcAddress(module, name.as_ptr()) else {
        return Err(Error::Input(format!(
            "ntdll is missing {}",
            String::from_utf8_lossy(&name[..name.len() - 1])
        )));
    };

    let handle = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
    if handle.is_null() {
        return Err(Error::Input(format!("could not open process {pid}")));
    }

    let f: extern "system" fn(isize) -> i32 = std::mem::transmute(proc_addr);
    let status = f(handle as isize);
    CloseHandle(handle);

    if status < 0 {
        return Err(Error::Input(format!("ntdll call failed for {pid}: {status:#x}")));
    }
    Ok(())
}

#[cfg(windows)]
fn suspend_one(pid: u32) -> Result<()> {
    unsafe { ntdll_call(b"NtSuspendProcess\0", pid) }
}

#[cfg(windows)]
fn resume_one(pid: u32) -> Result<()> {
    unsafe { ntdll_call(b"NtResumeProcess\0", pid) }
}

#[cfg(not(any(unix, windows)))]
pub fn find_game_pids() -> Vec<u32> {
    Vec::new()
}
#[cfg(not(any(unix, windows)))]
fn suspend_one(_pid: u32) -> Result<()> {
    Err(Error::NoBackend("process freeze is unsupported here".into()))
}
#[cfg(not(any(unix, windows)))]
fn resume_one(_pid: u32) -> Result<()> {
    Ok(())
}

/// Suspend every process that is the game. Returns how many were stopped.
///
/// All of them, not just one: Sober runs several processes under the same
/// name, and stopping only the first leaves the game running.
pub fn suspend_game() -> Result<usize> {
    let pids = find_game_pids();
    if pids.is_empty() {
        return Err(Error::Input("no running game found".into()));
    }

    let mut stopped = Vec::new();
    for pid in pids {
        match suspend_one(pid) {
            Ok(()) => stopped.push(pid),
            Err(e) => tracing::debug!(pid, error = %e, "could not suspend"),
        }
    }

    if stopped.is_empty() {
        return Err(Error::Input("found the game but could not suspend it".into()));
    }

    if let Ok(mut s) = SUSPENDED.lock() {
        for pid in &stopped {
            if !s.contains(pid) {
                s.push(*pid);
            }
        }
    }
    Ok(stopped.len())
}

/// Release everything we suspended. Called on engine stop, on the panic key,
/// and at shutdown, so a crash mid-freeze cannot leave the game hung.
pub fn resume_all() {
    let pids: Vec<u32> = SUSPENDED.lock().map(|s| s.clone()).unwrap_or_default();
    for pid in pids {
        if let Err(e) = resume_one(pid) {
            tracing::error!(pid, error = %e, "could not resume — the game may be stopped");
        }
    }
    if let Ok(mut s) = SUSPENDED.lock() {
        s.clear();
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
        assert_eq!(clamp_freeze(60_000), MAX_FREEZE_MS);
    }

    #[test]
    fn game_process_names_are_recognised_on_both_platforms() {
        assert!(is_game("sober"));
        assert!(is_game("sober\n"));
        assert!(is_game("RobloxPlayerBeta.exe"));
        assert!(is_game("RobloxPlayerBeta"));
        assert!(!is_game("bwrap"));
        assert!(!is_game("rojoin-v4"));
        assert!(!is_game(""));
    }

    #[test]
    fn scanning_processes_does_not_bail_early() {
        let _ = find_game_pids();
    }

    #[test]
    fn resume_all_is_safe_when_nothing_is_suspended() {
        resume_all();
        assert!(!anything_suspended());
    }

    #[test]
    fn suspending_with_no_game_reports_that_clearly() {
        if find_game_pids().is_empty() {
            let err = suspend_game().unwrap_err();
            assert!(err.to_string().contains("no running game"));
        }
    }
}
