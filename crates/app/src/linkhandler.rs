//! Registering RoJoin as the handler for `roblox://` links.
//!
//! Linux writes a `.desktop` file and points `xdg-mime` at it. Windows writes
//! the protocol under `HKCU\Software\Classes`, which needs no elevation —
//! per-user classes take precedence over the machine-wide ones for the logged-in
//! user, so this can claim `roblox://` without touching the Roblox install.

// All three are the Linux registration's business; Windows registers through
// the registry and touches none of it.
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
const DESKTOP_NAME: &str = "rojoin-v4-roblox.desktop";
#[cfg(unix)]
const MIME: &str = "x-scheme-handler/roblox";

#[cfg(unix)]
fn desktop_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".local/share/applications")
            .join(DESKTOP_NAME),
    )
}

pub fn is_registered() -> bool {
    #[cfg(unix)]
    {
        desktop_path().map(|p| p.exists()).unwrap_or(false)
    }
    #[cfg(windows)]
    {
        windows_impl::is_registered()
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

pub fn set_registered(on: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let Some(path) = desktop_path() else {
            return Err(std::io::Error::other("no HOME"));
        };

        if !on {
            if path.exists() {
                std::fs::remove_file(&path)?;
                let _ = std::process::Command::new("xdg-mime")
                    .args(["default", "", MIME])
                    .status();
            }
            return Ok(());
        }

        let exe = std::env::current_exe()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=RoJoin\n\
             Comment=Open Roblox links with RoJoin\n\
             Exec=\"{}\" %u\n\
             Terminal=false\n\
             NoDisplay=true\n\
             MimeType={MIME};\n",
            exe.display()
        );
        std::fs::write(&path, contents)?;

        let _ = std::process::Command::new("xdg-mime")
            .args(["default", DESKTOP_NAME, MIME])
            .status();
        let _ = std::process::Command::new("update-desktop-database")
            .arg(path.parent().unwrap_or(&path))
            .status();

        Ok(())
    }
    #[cfg(windows)]
    {
        windows_impl::set_registered(on)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = on;
        Err(std::io::Error::other(
            "link-handler registration is not implemented on this platform",
        ))
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    /// Per-user, so nothing here needs administrator rights. `HKCU\Software\
    /// Classes` shadows `HKLM` for this user, which is exactly the behaviour
    /// wanted: claim the scheme for whoever is signed in and leave the machine
    /// alone.
    const KEY: &str = r"HKCU\Software\Classes\roblox";
    const CMD_KEY: &str = r"HKCU\Software\Classes\roblox\shell\open\command";

    /// Same no-console treatment as everywhere else that shells out.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("reg")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .output()
    }

    pub fn is_registered() -> bool {
        // Checking the command key, not the parent: a half-written registration
        // with no command would report as registered and then do nothing.
        reg(&["query", CMD_KEY])
            .map(|o| {
                o.status.success()
                    && String::from_utf8_lossy(&o.stdout).to_lowercase().contains("rojoin")
            })
            .unwrap_or(false)
    }

    pub fn set_registered(on: bool) -> std::io::Result<()> {
        if !on {
            // Not being there is a successful removal.
            let _ = reg(&["delete", KEY, "/f"]);
            return Ok(());
        }

        let exe = std::env::current_exe()?;
        // Quoted, because Program Files has a space in it, and `%1` unquoted
        // would split a URI on one too.
        let command = format!("\"{}\" \"%1\"", exe.display());

        let steps: [&[&str]; 4] = [
            &["add", KEY, "/ve", "/t", "REG_SZ", "/d", "URL:Roblox Protocol", "/f"],
            &["add", KEY, "/v", "URL Protocol", "/t", "REG_SZ", "/d", "", "/f"],
            &["add", KEY, "/v", "FriendlyTypeName", "/t", "REG_SZ", "/d", "RoJoin", "/f"],
            &["add", CMD_KEY, "/ve", "/t", "REG_SZ", "/d", &command, "/f"],
        ];

        for args in steps {
            let out = reg(args)?;
            if !out.status.success() {
                return Err(std::io::Error::other(format!(
                    "reg {} failed: {}",
                    args.first().copied().unwrap_or("?"),
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }
        Ok(())
    }
}

/// Pull a place id (and optional server) out of a `roblox://` URI passed on the
/// command line.
pub fn parse_uri(uri: &str) -> Option<(i64, Option<String>)> {
    if !uri.starts_with("roblox://") && !uri.starts_with("roblox-player:") {
        return None;
    }

    let query = uri.split('?').nth(1).unwrap_or("");
    let mut place = None;
    let mut instance = None;

    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("placeId"), Some(v)) => place = v.parse::<i64>().ok(),
            (Some("gameInstanceId"), Some(v)) if !v.is_empty() => instance = Some(v.to_string()),
            _ => {}
        }
    }

    place.map(|p| (p, instance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_place_link() {
        assert_eq!(
            parse_uri("roblox://experiences/start?placeId=606849621"),
            Some((606849621, None))
        );
    }

    #[test]
    fn parses_a_server_link() {
        let (place, instance) =
            parse_uri("roblox://experiences/start?placeId=1&gameInstanceId=abc-123").unwrap();
        assert_eq!(place, 1);
        assert_eq!(instance.as_deref(), Some("abc-123"));
    }

    #[test]
    fn ignores_unrelated_uris() {
        assert!(parse_uri("https://roblox.com/games/1").is_none());
        assert!(parse_uri("").is_none());
    }

    #[test]
    fn a_link_without_a_place_is_not_launchable() {
        assert!(parse_uri("roblox://experiences/start?foo=bar").is_none());
    }

    #[test]
    fn an_empty_instance_id_is_treated_as_absent() {
        let (_, instance) = parse_uri("roblox://start?placeId=5&gameInstanceId=").unwrap();
        assert!(instance.is_none());
    }
}
