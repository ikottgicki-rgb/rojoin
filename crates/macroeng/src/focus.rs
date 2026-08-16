//! Is the game the focused window?
//!
//! Without this, a global hotkey fires wherever you are — press F3 while
//! typing in a browser and your game freezes. Gating on focus is the single
//! most important safety property the macro tab has.
//!
//! Detection is best-effort and platform-dependent:
//!   * Hyprland — `hyprctl activewindow -j`
//!   * Sway     — `swaymsg -t get_tree`
//!   * X11      — `xprop` on the active window
//!   * Windows  — `GetForegroundWindow` + the owning process name
//!
//! **When we genuinely cannot tell, we allow the macro.** A gate that silently
//! blocks everything on an unsupported compositor is worse than one that
//! occasionally lets a keystroke through — the user would just conclude the
//! whole feature is broken.

use std::process::{Command, Stdio};

/// Window classes and process names that mean "the game".
const NEEDLES: &[&str] = &["sober", "roblox", "robloxplayerbeta", "vinegar"];

fn looks_like_game(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    NEEDLES.iter().any(|n| s.contains(n))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Game,
    Other,
    /// No way to tell on this system.
    Unknown,
}

impl Focus {
    /// Should a macro be allowed to run?
    ///
    /// `Unknown` allows, deliberately — see the module docs.
    pub fn allows_macros(self) -> bool {
        !matches!(self, Focus::Other)
    }
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(unix)]
pub fn current() -> Focus {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        if let Some(json) = run("hyprctl", &["activewindow", "-j"]) {
            return if looks_like_game(&json) { Focus::Game } else { Focus::Other };
        }
    }

    if std::env::var_os("SWAYSOCK").is_some() {
        if let Some(json) = run("swaymsg", &["-t", "get_tree"]) {
            if let Some(idx) = json.find("\"focused\":true") {
                let window = &json[idx.saturating_sub(400)..idx];
                return if looks_like_game(window) { Focus::Game } else { Focus::Other };
            }
        }
    }

    if std::env::var_os("DISPLAY").is_some() {
        if let Some(root) = run("xprop", &["-root", "_NET_ACTIVE_WINDOW"]) {
            if let Some(id) = root.split_whitespace().last() {
                if let Some(props) = run("xprop", &["-id", id, "WM_CLASS"]) {
                    return if looks_like_game(&props) { Focus::Game } else { Focus::Other };
                }
            }
        }
    }

    Focus::Unknown
}

#[cfg(windows)]
pub fn current() -> Focus {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return Focus::Unknown;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return Focus::Unknown;
        }

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return Focus::Unknown;
        }

        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);

        if ok == 0 {
            return Focus::Unknown;
        }

        let name = String::from_utf16_lossy(&buf[..len as usize]);
        if looks_like_game(&name) { Focus::Game } else { Focus::Other }
    }
}

#[cfg(not(any(unix, windows)))]
pub fn current() -> Focus {
    Focus::Unknown
}

/// Whether focus can be determined at all here, so the UI can say so rather
/// than offering a toggle that does nothing.
pub fn is_detectable() -> bool {
    current() != Focus::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_windows_are_recognised() {
        assert!(looks_like_game("class: org.vinegarhq.Sober"));
        assert!(looks_like_game(r#"{"class":"sober","title":"Roblox"}"#));
        assert!(looks_like_game(r"C:\Program Files\Roblox\RobloxPlayerBeta.exe"));
    }

    #[test]
    fn other_windows_are_not() {
        assert!(!looks_like_game(r#"{"class":"firefox","title":"GitHub"}"#));
        assert!(!looks_like_game("class: Alacritty"));
        assert!(!looks_like_game(""));
    }

    #[test]
    fn unknown_focus_allows_macros_rather_than_blocking_everything() {
        assert!(Focus::Unknown.allows_macros());
        assert!(Focus::Game.allows_macros());
        assert!(!Focus::Other.allows_macros());
    }

    #[test]
    fn detection_does_not_panic_on_this_machine() {
        let _ = current();
    }
}
