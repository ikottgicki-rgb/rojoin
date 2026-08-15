//! Registering RoJoin as the handler for `roblox://` links.
//!
//! Linux: writes a `.desktop` file and points `xdg-mime` at it. Windows: needs
//! registry writes, which are deliberately not done yet — the toggle reports
//! its real state rather than claiming success.

use std::path::PathBuf;

const DESKTOP_NAME: &str = "rojoin-v4-roblox.desktop";
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
    #[cfg(not(unix))]
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

        // Exec is quoted: an unquoted path with a space silently breaks the
        // handler, and it fails at click time rather than at registration.
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
    #[cfg(not(unix))]
    {
        let _ = on;
        Err(std::io::Error::other(
            "link-handler registration is not implemented on this platform",
        ))
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
        // Roblox emits a trailing empty gameInstanceId sometimes; passing it
        // through would ask for a server named "".
        let (_, instance) = parse_uri("roblox://start?placeId=5&gameInstanceId=").unwrap();
        assert!(instance.is_none());
    }
}
