//! Start RoJoin when the user logs in.
//!
//! Two entirely different mechanisms, one interface:
//!   * **Linux** — a `.desktop` file in `~/.config/autostart`, which is the
//!     XDG convention every desktop honours.
//!   * **Windows** — a value under `HKCU\…\CurrentVersion\Run`, written with
//!     `reg.exe` rather than a registry crate so the dependency list and the
//!     cross-build stay as they are.
//!
//! Both point at the binary's *current* location, so the entry is rewritten on
//! every enable rather than trusted to still be right — an AppImage in
//! particular is routinely moved after first run.

/// Where the running binary really lives.
///
/// Inside an AppImage `current_exe()` is the read-only squashfs mount under
/// /tmp, which is gone by the next boot. `APPIMAGE` holds the path the user
/// actually launched.
fn exe_path() -> Option<std::path::PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        let path = std::path::PathBuf::from(appimage);
        if path.is_file() {
            return Some(path);
        }
    }
    std::env::current_exe().ok()
}

/// Split from `desktop_file` so it can be tested without setting environment
/// variables — `set_var` is process-global, and Rust runs tests in threads, so a
/// test that mutates the environment quietly changes where *other* tests look.
#[cfg(not(windows))]
fn desktop_file_in(config_home: &std::path::Path) -> std::path::PathBuf {
    config_home.join("autostart").join("rojoin-v4.desktop")
}

#[cfg(not(windows))]
fn desktop_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(desktop_file_in(&base))
}

#[cfg(not(windows))]
pub fn is_enabled() -> bool {
    desktop_file().map(|p| p.is_file()).unwrap_or(false)
}

#[cfg(not(windows))]
pub fn set(enabled: bool) -> anyhow::Result<()> {
    let Some(path) = desktop_file() else {
        anyhow::bail!("no config directory to write an autostart entry into")
    };

    if !enabled {
        // Already absent is success, not an error.
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }

    let Some(exe) = exe_path() else {
        anyhow::bail!("could not work out where RoJoin is installed")
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // `--hidden` so a login does not throw a window in the user's face; the
    // point of starting is to keep tracking, not to be looked at.
    std::fs::write(
        &path,
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=RoJoin\n\
             Comment=Native Roblox client\n\
             Exec={} --hidden\n\
             Icon=rojoin-v4\n\
             Terminal=false\n\
             Categories=Game;\n\
             X-GNOME-Autostart-enabled=true\n",
            exe.display()
        ),
    )?;
    Ok(())
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: this build goes to lengths to have no console, and
    // shelling out must not undo that with a flash of one.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

#[cfg(windows)]
pub fn is_enabled() -> bool {
    reg(&["query", RUN_KEY, "/v", "RoJoin"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn set(enabled: bool) -> anyhow::Result<()> {
    if !enabled {
        // A missing value is not a failure to remove it.
        let _ = reg(&["delete", RUN_KEY, "/v", "RoJoin", "/f"]);
        return Ok(());
    }

    let Some(exe) = exe_path() else {
        anyhow::bail!("could not work out where RoJoin is installed")
    };

    // Quoted: Program Files has a space in it, and an unquoted path there is
    // the classic way this silently stops working.
    let value = format!("\"{}\" --hidden", exe.display());
    let out = reg(&["add", RUN_KEY, "/v", "RoJoin", "/t", "REG_SZ", "/d", &value, "/f"])?;
    if !out.status.success() {
        anyhow::bail!("reg add failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Was the process started by the autostart entry?
///
/// Checked rather than assumed from a setting, so a hand-run launch always shows
/// its window even with autostart on.
pub fn started_hidden() -> bool {
    std::env::args().any(|a| a == "--hidden")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_is_opt_in_per_launch() {
        // Nothing passes --hidden in a test run, so this must be false rather
        // than reading a stored setting.
        assert!(!started_hidden());
    }

    #[cfg(not(windows))]
    #[test]
    fn the_entry_lands_in_the_xdg_autostart_directory() {
        let path = desktop_file_in(std::path::Path::new("/tmp/cfg"));
        assert_eq!(path, std::path::Path::new("/tmp/cfg/autostart/rojoin-v4.desktop"));
    }

    #[cfg(not(windows))]
    #[test]
    fn the_real_path_follows_xdg_config_home() {
        // Only asserts the shape, without touching the environment.
        if let Some(p) = desktop_file() {
            assert!(p.parent().unwrap().ends_with("autostart"));
            assert_eq!(p.file_name().unwrap(), "rojoin-v4.desktop");
        }
    }
}
