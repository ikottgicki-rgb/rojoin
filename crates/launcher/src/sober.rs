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
/// Launch a `roblox://` URI through Sober.
///
/// Runs the flatpak directly rather than going through `xdg-open`. The scheme
/// handler on a given desktop may point anywhere — on this box it opened the
/// link in Chromium — so relying on it makes Play a coin flip.
///
/// Null stdio and a separate process group are not optional: Sober is very
/// chatty on stdout, and inheriting a pipe makes it block on write() and hang
/// at startup with no window.
pub fn launch_uri(uri: &str) -> Result<()> {
    tracing::info!(uri, "launching via Sober");

    let mut cmd = Command::new("flatpak");
    cmd.args(["run", FLATPAK_ID, uri])
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
        .map_err(|e| Error::Launch(format!("could not start Sober: {e}")))?;

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

// ---------------------------------------------------------------- config ---
//
// Sober keeps its client settings in `config/sober/config.json`, which is the
// closest Linux equivalent of what Bloxstrap exposes on Windows: engine
// FastFlags plus a handful of real client toggles.

pub fn config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::Path::new(&home)
            .join(".var/app")
            .join(FLATPAK_ID)
            .join("config/sober/config.json"),
    )
}

/// Strip `//` line comments so the file can be parsed as JSON.
///
/// Sober ships its config with a comment header warning you not to edit it by
/// hand, which means the file is *not* strict JSON and `serde_json` refuses it
/// outright. Quotes are tracked so a `//` inside a string value survives.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut in_string = false;
        let mut escaped = false;
        let mut cut = line.len();
        let bytes: Vec<char> = line.chars().collect();
        for i in 0..bytes.len() {
            let c = bytes[i];
            if escaped {
                escaped = false;
                continue;
            }
            match c {
                '\\' if in_string => escaped = true,
                '"' => in_string = !in_string,
                '/' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == '/' => {
                    cut = i;
                    break;
                }
                _ => {}
            }
        }
        out.push_str(&bytes[..cut].iter().collect::<String>());
        out.push('\n');
    }
    out
}

/// Sober's settings, as far as RoJoin exposes them.
pub fn read_config() -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(config_path()?).ok()?;
    serde_json::from_str(&strip_comments(&text)).ok()
}

/// Change one top-level key, leaving every other setting untouched.
///
/// Refuses while Sober is running: it reads this file at startup and rewrites
/// it on exit, so an edit mid-session is silently discarded.
pub fn set_config_key(key: &str, value: serde_json::Value) -> Result<()> {
    if is_running() {
        return Err(Error::Launch(
            "close Roblox first — Sober overwrites its settings when it exits".into(),
        ));
    }

    let path = config_path().ok_or_else(|| Error::Launch("no HOME set".into()))?;
    let mut config = read_config().unwrap_or_else(|| serde_json::json!({}));

    let Some(map) = config.as_object_mut() else {
        return Err(Error::Launch("Sober's config is not an object".into()));
    };
    map.insert(key.to_string(), value);

    let pretty = serde_json::to_string_pretty(&config)
        .map_err(|e| Error::Launch(format!("could not encode the config: {e}")))?;

    // Write beside the target then rename, so a crash mid-write cannot leave
    // Sober with a truncated config it refuses to start from.
    let tmp = path.with_extension("json.rojoin-tmp");
    std::fs::write(&tmp, pretty).map_err(|e| Error::Launch(format!("could not write: {e}")))?;
    std::fs::rename(&tmp, &path).map_err(|e| Error::Launch(format!("could not replace: {e}")))?;
    Ok(())
}

/// The engine FastFlags currently set, as (name, value) pairs.
pub fn fflags() -> Vec<(String, String)> {
    let Some(config) = read_config() else { return Vec::new() };
    let Some(map) = config.get("fflags").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = map
        .iter()
        .filter(|(k, _)| k.as_str() != PLACEHOLDER_FLAG)
        .map(|(k, v)| {
            let shown = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), shown)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Replace Sober's entire flag set with the one given.
///
/// RoJoin owns flags per account, so Sober's copy is derived: it is overwritten
/// wholesale before a launch rather than edited in place. Sober re-creates its
/// own `FFlagExample` placeholder whenever it rewrites the file, so that name is
/// dropped here — it does nothing, and letting it through makes it look as
/// though the user set it.
pub fn write_fflags(flags: &[(String, String)]) -> Result<()> {
    let mut obj = serde_json::Map::new();
    for (name, value) in flags {
        if name == PLACEHOLDER_FLAG {
            continue;
        }
        obj.insert(name.clone(), typed_flag(value));
    }
    set_config_key("fflags", serde_json::Value::Object(obj))
}

/// Sober writes this into a fresh config as an example. It has no effect.
pub const PLACEHOLDER_FLAG: &str = "FFlagExample";

/// Add or replace a FastFlag. An empty value removes it.
///
/// Values are typed the way the engine expects: `true`/`false` as booleans,
/// digits as numbers, everything else as a string. Sending `"true"` as a string
/// where a bool is wanted is silently ignored by the engine.
pub fn set_fflag(name: &str, value: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Launch("a flag needs a name".into()));
    }

    let mut config = read_config().unwrap_or_else(|| serde_json::json!({}));
    let map = config
        .as_object_mut()
        .ok_or_else(|| Error::Launch("Sober's config is not an object".into()))?;

    let flags = map
        .entry("fflags")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::Launch("fflags is not an object".into()))?;

    let trimmed = value.trim();
    if trimmed.is_empty() {
        flags.remove(name);
    } else {
        flags.insert(name.to_string(), typed_flag(trimmed));
    }

    let flags = serde_json::Value::Object(flags.clone());
    set_config_key("fflags", flags)
}

fn typed_flag(value: &str) -> serde_json::Value {
    match value {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        other => match other.parse::<i64>() {
            Ok(n) => serde_json::Value::from(n),
            Err(_) => serde_json::Value::String(other.to_string()),
        },
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn the_comment_header_sober_ships_is_stripped_and_parses() {
        // Trimmed from the real file, which is not valid JSON as written.
        let real = "// !!! STOP !!!\n\
                    // This file is not meant to be edited by hand\n\
                    // -------------------------------------------\n\
                    {\n    \"use_opengl\": true,\n    \"fflags\": { \"FFlagExample\": true }\n}\n";
        let cleaned = strip_comments(real);
        let parsed: serde_json::Value =
            serde_json::from_str(&cleaned).expect("should parse once comments are gone");
        assert_eq!(parsed["use_opengl"], serde_json::json!(true));
        assert_eq!(parsed["fflags"]["FFlagExample"], serde_json::json!(true));
    }

    #[test]
    fn a_double_slash_inside_a_string_is_not_a_comment() {
        let text = "{\"url\": \"https://example.com/x\"}";
        assert_eq!(strip_comments(text).trim(), text);
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        let text = r#"{"a": "say \"hi\" // not a comment"}"#;
        assert!(strip_comments(text).contains("not a comment"));
    }

    #[test]
    fn flags_are_typed_the_way_the_engine_expects() {
        // A bool sent as the string "true" is ignored by the engine.
        assert_eq!(typed_flag("true"), serde_json::json!(true));
        assert_eq!(typed_flag("false"), serde_json::json!(false));
        assert_eq!(typed_flag("240"), serde_json::json!(240));
        assert_eq!(typed_flag("Balanced"), serde_json::json!("Balanced"));
    }

    #[test]
    fn stripping_never_loses_a_line() {
        let text = "{\n// gone\n\"a\": 1\n}";
        assert_eq!(strip_comments(text).lines().count(), text.lines().count());
    }
}
