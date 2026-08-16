//! Windows launch path: the official Roblox client.
//!
//! Windows has no cookie-swap trick, so joining as a chosen account goes
//! through an **authentication ticket**: a short-lived, single-use token minted
//! from the account's cookie and handed to the client in the launch URI. That
//! is what makes multi-account launching work at all on Windows.
//!
//! The URI shape the client expects:
//! ```text
//! roblox-player:1+launchmode:play+gameinfo:<ticket>+launchtime:<ms>
//!   +placelauncherurl:<url-encoded PlaceLauncher.ashx url>
//!   +browsertrackerid:<id>+robloxLocale:en_us+gameLocale:en_us
//! ```

use crate::Result;

const PLACE_LAUNCHER: &str = "https://assetgame.roblox.com/game/PlaceLauncher.ashx";

/// Build the PlaceLauncher URL for a normal join, a specific server, or a
/// private server.
pub fn place_launcher_url(place_id: i64, job_id: Option<&str>, access_code: Option<&str>) -> String {
    match (job_id, access_code) {
        (_, Some(code)) => format!(
            "{PLACE_LAUNCHER}?request=RequestPrivateGame&placeId={place_id}&accessCode={code}"
        ),
        (Some(job), _) => format!(
            "{PLACE_LAUNCHER}?request=RequestGameJob&placeId={place_id}&gameId={job}"
        ),
        _ => format!("{PLACE_LAUNCHER}?request=RequestGame&placeId={place_id}"),
    }
}

/// Assemble the full `roblox-player:` URI.
pub fn launch_uri(ticket: &str, place_launcher: &str, launch_time_ms: i64, tracker_id: i64) -> String {
    format!(
        "roblox-player:1+launchmode:play+gameinfo:{ticket}+launchtime:{launch_time_ms}\
         +placelauncherurl:{}+browsertrackerid:{tracker_id}+robloxLocale:en_us+gameLocale:en_us",
        percent_encode(place_launcher)
    )
}

/// Percent-encode the PlaceLauncher URL so its `?`, `&` and `=` do not collide
/// with the `+`-separated launch-URI grammar.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(windows)]
pub fn open_uri(uri: &str) -> Result<()> {
    use std::process::{Command, Stdio};

    tracing::info!("launching via Roblox client");

    Command::new("cmd")
        .args(["/C", "start", "", uri])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| crate::Error::Launch(format!("could not start Roblox client: {e}")))?;

    Ok(())
}

#[cfg(not(windows))]
pub fn open_uri(_uri: &str) -> Result<()> {
    Err(crate::Error::Launch(
        "the Windows client launcher is not available on this platform".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_join_requests_a_game() {
        let u = place_launcher_url(606849621, None, None);
        assert!(u.contains("request=RequestGame"));
        assert!(u.contains("placeId=606849621"));
    }

    #[test]
    fn specific_server_requests_a_game_job() {
        let u = place_launcher_url(1, Some("abc-def"), None);
        assert!(u.contains("request=RequestGameJob"));
        assert!(u.contains("gameId=abc-def"));
    }

    #[test]
    fn private_server_takes_priority_over_job_id() {
        let u = place_launcher_url(1, Some("job"), Some("code123"));
        assert!(u.contains("RequestPrivateGame"));
        assert!(u.contains("accessCode=code123"));
        assert!(!u.contains("RequestGameJob"));
    }

    #[test]
    fn launch_uri_encodes_the_inner_url() {
        let inner = place_launcher_url(606849621, None, None);
        let uri = launch_uri("TICKET", &inner, 1234, 99);

        assert!(!uri.contains("?request="), "inner ? leaked into the launch uri");
        assert!(uri.contains("%3Frequest%3D"));
        assert!(uri.contains("gameinfo:TICKET"));
        assert!(uri.contains("launchtime:1234"));
        assert!(uri.contains("browsertrackerid:99"));
    }
}
