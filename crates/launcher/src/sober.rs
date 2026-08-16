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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

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

const COOKIE_NAME: &str = ".ROBLOSECURITY";

pub fn cookie_path() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("cookies"))
}

/// Split a cookie jar line into (name, value) pairs, preserving order.
fn split_jar(jar: &str) -> Vec<(String, String)> {
    jar.split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            match part.split_once('=') {
                Some((k, v)) => Some((k.trim().to_string(), v.to_string())),
                None => Some((part.to_string(), String::new())),
            }
        })
        .collect()
}

fn join_jar(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{k}={v}") })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The account Sober is currently signed into.
pub fn read_cookie() -> Option<String> {
    let path = cookie_path()?;
    let jar = std::fs::read_to_string(path).ok()?;
    split_jar(&jar)
        .into_iter()
        .find(|(k, _)| k == COOKIE_NAME)
        .map(|(_, v)| v)
        .filter(|v| !v.is_empty())
}

/// Point Sober at a different account.
///
/// Returns `Ok(false)` when it already holds this cookie, so a caller can skip
/// the "close Roblox first" prompt when nothing needs to change.
///
/// Refuses while the game is running: Sober rewrites this file on exit, so a
/// swap underneath a live session is silently undone and the user is left
/// wondering why they joined as the wrong account anyway.
pub fn set_cookie(cookie: &str) -> Result<bool> {
    if cookie.is_empty() {
        return Err(Error::Launch("refusing to write an empty cookie".into()));
    }

    let Some(path) = cookie_path() else {
        return Err(Error::SoberMissing);
    };

    let jar = std::fs::read_to_string(&path)
        .map_err(|e| Error::Launch(format!("could not read Sober's cookies: {e}")))?;

    let mut pairs = split_jar(&jar);
    if pairs
        .iter()
        .any(|(k, v)| k == COOKIE_NAME && v == cookie)
    {
        return Ok(false);
    }

    if is_running() {
        return Err(Error::Launch(
            "close Roblox first — Sober rewrites its session on exit, so switching \
             accounts while it is running would be undone"
                .into(),
        ));
    }

    match pairs.iter_mut().find(|(k, _)| k == COOKIE_NAME) {
        Some(entry) => entry.1 = cookie.to_string(),
        None => pairs.push((COOKIE_NAME.to_string(), cookie.to_string())),
    }

    let backup = path.with_extension("rojoin-backup");
    let _ = std::fs::copy(&path, &backup);

    let tmp = path.with_extension("rojoin-tmp");
    std::fs::write(&tmp, join_jar(&pairs))
        .map_err(|e| Error::Launch(format!("could not stage Sober's cookies: {e}")))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| Error::Launch(format!("could not replace Sober's cookies: {e}")))?;

    tracing::info!("switched Sober to the selected account");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAR: &str = "rbxas=abc; GuestData=UserID=-1; .ROBLOSECURITY=OLDVALUE; path=/";

    #[test]
    fn the_cookie_is_found_among_the_others() {
        let found = split_jar(JAR)
            .into_iter()
            .find(|(k, _)| k == COOKIE_NAME)
            .map(|(_, v)| v);
        assert_eq!(found.as_deref(), Some("OLDVALUE"));
    }

    #[test]
    fn replacing_the_cookie_preserves_every_other_field() {
        let mut pairs = split_jar(JAR);
        pairs.iter_mut().find(|(k, _)| k == COOKIE_NAME).unwrap().1 = "NEW".into();
        let out = join_jar(&pairs);

        assert!(out.contains("rbxas=abc"));
        assert!(out.contains("GuestData=UserID=-1"));
        assert!(out.contains(".ROBLOSECURITY=NEW"));
        assert!(!out.contains("OLDVALUE"));
        assert!(out.contains("path=/"));
    }

    #[test]
    fn values_containing_equals_survive_a_round_trip() {
        let pairs = split_jar("GuestData=UserID=-634510552");
        assert_eq!(pairs[0].0, "GuestData");
        assert_eq!(pairs[0].1, "UserID=-634510552");
        assert_eq!(join_jar(&pairs), "GuestData=UserID=-634510552");
    }

    #[test]
    fn an_empty_cookie_is_refused() {
        assert!(set_cookie("").is_err());
    }
}
