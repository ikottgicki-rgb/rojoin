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
//!   +channel:+LaunchExp:InApp
//! ```
//!
//! Every one of those trailing fields matters. An earlier version stopped after
//! `gameLocale` and hardcoded `browsertrackerid:1`, and joins failed: `channel:`
//! (empty, meaning the production deploy) and `LaunchExp:InApp` are what the
//! website itself sends, and a tracker id of `1` is not a value any real
//! browser session would produce.

use std::sync::OnceLock;

use crate::{JoinRequest, Result};

const PLACE_LAUNCHER: &str = "https://assetgame.roblox.com/game/PlaceLauncher.ashx";

/// A plausible browser-tracker id, generated once per process.
///
/// Roblox logs this against the join attempt and expects something shaped like
/// a real browser session — an 11-to-12 digit number. It is an identifier, not
/// a secret, so deriving it from the clock is enough and saves pulling in a
/// random-number dependency. Stable for the lifetime of the process, the way a
/// browser tab's would be.
pub fn tracker_id() -> i64 {
    static ID: OnceLock<i64> = OnceLock::new();

    *ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as i64)
            .unwrap_or(0);

        // Mirrors the shape the website sends: a 6-digit block then a 6-digit
        // block, concatenated.
        let high = 100_000 + (nanos % 75_000);
        let low = 100_000 + ((nanos / 75_000) % 800_000);
        high * 1_000_000 + low
    })
}

/// Build the PlaceLauncher URL for a normal join, a specific server, a private
/// (VIP) server link, or a reserved server.
///
/// `browserTrackerId` and `isPlayTogetherGame` are not optional decoration —
/// PlaceLauncher expects both, and leaving them off is a join that never
/// resolves.
pub fn place_launcher_url(req: &JoinRequest, tracker_id: i64) -> String {
    // A private server is identified either by a share `linkCode` (what a
    // roblox.com/games/...?privateServerLinkCode=... URL carries) or by a
    // reserved-server `accessCode`. They are different parameters and are not
    // interchangeable: sending a link code as `accessCode` is a join Roblox
    // simply refuses.
    if req.link_code.is_some() || req.access_code.is_some() {
        let mut url = format!(
            "{PLACE_LAUNCHER}?request=RequestPrivateGame&browserTrackerId={tracker_id}\
             &placeId={}",
            req.place_id
        );
        if let Some(code) = &req.access_code {
            url.push_str(&format!("&accessCode={code}"));
        }
        if let Some(code) = &req.link_code {
            url.push_str(&format!("&linkCode={code}"));
        }
        return url;
    }

    match &req.job_id {
        Some(job) => format!(
            "{PLACE_LAUNCHER}?request=RequestGameJob&browserTrackerId={tracker_id}\
             &placeId={}&gameId={job}&isPlayTogetherGame=false",
            req.place_id
        ),
        None => format!(
            "{PLACE_LAUNCHER}?request=RequestGame&browserTrackerId={tracker_id}\
             &placeId={}&isPlayTogetherGame=false",
            req.place_id
        ),
    }
}

/// Assemble the full `roblox-player:` URI.
pub fn launch_uri(ticket: &str, place_launcher: &str, launch_time_ms: i64, tracker_id: i64) -> String {
    format!(
        "roblox-player:1+launchmode:play+gameinfo:{ticket}+launchtime:{launch_time_ms}\
         +placelauncherurl:{}+browsertrackerid:{tracker_id}+robloxLocale:en_us\
         +gameLocale:en_us+channel:+LaunchExp:InApp",
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

/// Hand the URI to the shell.
///
/// `ShellExecuteW` directly, rather than `cmd /C start`: the URI is dense with
/// `%XX` escapes, and cmd applies its own `%VAR%` expansion and quoting rules
/// to whatever it is given. It also spawns a console window, which is exactly
/// what the rest of this build goes to lengths to avoid.
#[cfg(windows)]
pub fn open_uri(uri: &str) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    tracing::info!("launching via Roblox client");

    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };

    let verb = wide("open");
    let target = wide(uri);

    // Anything above 32 is success. Below that the return value is a legacy
    // WinExec error code; 2 and 3 both mean "nothing is registered to handle
    // this", which for `roblox-player:` means the Roblox client is not
    // installed.
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL as i32,
        ) as isize
    };

    if rc > 32 {
        return Ok(());
    }

    Err(crate::Error::Launch(match rc {
        2 | 3 => "Windows has nothing registered for roblox-player: links. \
                  Install the Roblox client and open a game from roblox.com \
                  once, then try again."
            .into(),
        _ => format!("Windows refused to start the Roblox client (code {rc})"),
    }))
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

    const BTID: i64 = 123456789012;

    #[test]
    fn normal_join_requests_a_game() {
        let u = place_launcher_url(&JoinRequest::place(606849621), BTID);
        assert!(u.contains("request=RequestGame&"));
        assert!(u.contains("placeId=606849621"));
        assert!(u.contains("isPlayTogetherGame=false"));
        assert!(u.contains("browserTrackerId=123456789012"));
    }

    #[test]
    fn specific_server_requests_a_game_job() {
        let u = place_launcher_url(&JoinRequest::place(1).server("abc-def"), BTID);
        assert!(u.contains("request=RequestGameJob"));
        assert!(u.contains("gameId=abc-def"));
        assert!(u.contains("browserTrackerId=123456789012"));
    }

    #[test]
    fn a_share_link_travels_as_link_code_not_access_code() {
        let u = place_launcher_url(&JoinRequest::place(1).private_link("shareXYZ"), BTID);
        assert!(u.contains("request=RequestPrivateGame"));
        assert!(u.contains("linkCode=shareXYZ"));
        assert!(
            !u.contains("accessCode="),
            "a share link is not a reserved-server access code"
        );
    }

    #[test]
    fn a_reserved_server_travels_as_access_code() {
        let u = place_launcher_url(&JoinRequest::place(1).reserved("code123"), BTID);
        assert!(u.contains("request=RequestPrivateGame"));
        assert!(u.contains("accessCode=code123"));
        assert!(!u.contains("linkCode="));
    }

    #[test]
    fn a_private_server_outranks_a_job_id() {
        let u = place_launcher_url(&JoinRequest::place(1).server("job").reserved("code123"), BTID);
        assert!(u.contains("RequestPrivateGame"));
        assert!(!u.contains("RequestGameJob"));
    }

    #[test]
    fn launch_uri_encodes_the_inner_url() {
        let inner = place_launcher_url(&JoinRequest::place(606849621), BTID);
        let uri = launch_uri("TICKET", &inner, 1234, BTID);

        assert!(!uri.contains("?request="), "inner ? leaked into the launch uri");
        assert!(uri.contains("%3Frequest%3D"));
        assert!(uri.contains("gameinfo:TICKET"));
        assert!(uri.contains("launchtime:1234"));
        assert!(uri.contains("browsertrackerid:123456789012"));
    }

    #[test]
    fn launch_uri_carries_the_fields_the_website_sends() {
        let inner = place_launcher_url(&JoinRequest::place(1), BTID);
        let uri = launch_uri("T", &inner, 1, BTID);

        // Roblox rejects the launch without these two; they were the missing
        // half of "joining does nothing".
        assert!(uri.contains("+channel:"));
        assert!(uri.ends_with("+LaunchExp:InApp"));
    }

    #[test]
    fn tracker_id_looks_like_a_browser_session() {
        let id = tracker_id();
        assert_eq!(tracker_id(), id, "must be stable within a process");
        let digits = id.to_string().len();
        assert!((11..=12).contains(&digits), "got {digits} digits: {id}");
    }
}
