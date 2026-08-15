//! Linux launch path: Sober (the Roblox flatpak).
//!
//! Two things here are not optional, both learned the expensive way:
//!
//! 1. **Null stdio and a separate process group.** Sober is extremely chatty on
//!    stdout. If it inherits a pipe (which it does whenever RoJoin's own stdout
//!    is captured by a terminal or desktop session), its write() blocks once the
//!    pipe buffer fills and Sober hangs at startup with no window — it gets as
//!    far as a valid deep link and then simply stops. It looks exactly like
//!    "clicking play did nothing".
//!
//! 2. **Detect via `flatpak ps`, not /proc.** A running Sober's cmdline is
//!    `bwrap … -- sober -- …`, which matches none of the obvious needles.

use std::process::{Command, Stdio};

use crate::{Error, Result};

pub const FLATPAK_ID: &str = "org.vinegarhq.Sober";

pub fn is_installed() -> bool {
    Command::new("flatpak")
        .args(["info", FLATPAK_ID])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Is a game currently running? Used to warn before an account switch, and to
/// drive playtime tracking.
pub fn is_running() -> bool {
    Command::new("flatpak")
        .args(["ps", "--columns=application"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(FLATPAK_ID))
        .unwrap_or(false)
}

/// Launch a `roblox://` URI.
pub fn launch_uri(uri: &str) -> Result<()> {
    tracing::info!(uri, "launching via Sober");

    let mut cmd = Command::new("xdg-open");
    cmd.arg(uri)
        // See the module docs: inheriting a pipe here hangs Sober at startup.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Our own session, so quitting RoJoin never takes the game down with it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    // The webview-era environment variables must never reach the game engine.
    cmd.env_remove("WEBKIT_DISABLE_DMABUF_RENDERER")
        .env_remove("WEBKIT_DISABLE_COMPOSITING_MODE")
        .env_remove("GSK_RENDERER");

    cmd.spawn()
        .map_err(|e| Error::Launch(format!("could not spawn xdg-open: {e}")))?;

    Ok(())
}

/// Sober keeps the active account's cookie in plaintext. Reading it lets us
/// offer "import the account you're already signed into" on first run.
pub fn data_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home)
        .join(".var/app")
        .join(FLATPAK_ID)
        .join("data/sober");
    path.exists().then_some(path)
}
