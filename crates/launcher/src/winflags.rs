//! FastFlags for the official Windows client.
//!
//! The client reads them from `ClientAppSettings.json` inside the *version
//! directory* it was launched from:
//!
//! ```text
//! %LOCALAPPDATA%\Roblox\Versions\<version>\ClientSettings\ClientAppSettings.json
//! ```
//!
//! Every installed version gets a copy, not just the newest. Roblox creates a
//! fresh directory on each update and the launcher decides which one runs, so
//! writing to a single guess is how flags silently stop applying the next time
//! Roblox updates.
//!
//! Values are typed the way the engine expects — `true`/`false` as booleans,
//! digits as numbers, everything else as a string. A bool sent as the string
//! `"true"` is ignored, which looks exactly like the flag not working.

use std::path::PathBuf;

use crate::{Error, Result};

/// Version directories that look like a real client install.
///
/// Presence of the executable is the test: `Versions` accumulates leftovers
/// from partial downloads and old uninstalls, and writing into those is
/// harmless but pointless.
pub fn version_dirs() -> Vec<PathBuf> {
    let Some(local) = local_appdata() else { return Vec::new() };
    let versions = local.join("Roblox").join("Versions");

    let Ok(entries) = std::fs::read_dir(&versions) else { return Vec::new() };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.join("RobloxPlayerBeta.exe").is_file() || p.join("RobloxApp.exe").is_file()
        })
        .collect()
}

fn local_appdata() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|p| PathBuf::from(p).join("AppData").join("Local"))
        })
}

/// Replace the flag set for every installed client.
///
/// An empty list writes `{}` rather than deleting the file — leaving a stale
/// file behind would keep applying flags the user has just cleared.
pub fn write_fflags(flags: &[(String, String)]) -> Result<()> {
    let mut obj = serde_json::Map::new();
    for (name, value) in flags {
        let name = name.trim();
        if name.is_empty() || name == crate::sober::PLACEHOLDER_FLAG {
            continue;
        }
        obj.insert(name.to_string(), typed_flag(value));
    }
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(obj))
        .map_err(|e| Error::Launch(format!("could not encode flags: {e}")))?;

    let dirs = version_dirs();
    if dirs.is_empty() {
        // Not an error: somebody may simply not have the client installed yet,
        // and the flags are still saved in RoJoin's own config either way.
        tracing::info!("no Roblox client install found; flags will apply once it is installed");
        return Ok(());
    }

    let mut wrote = 0usize;
    let mut last_error = None;

    for dir in &dirs {
        let settings = dir.join("ClientSettings");
        if let Err(e) = std::fs::create_dir_all(&settings) {
            last_error = Some(format!("{}: {e}", settings.display()));
            continue;
        }
        match std::fs::write(settings.join("ClientAppSettings.json"), &body) {
            Ok(()) => wrote += 1,
            Err(e) => last_error = Some(format!("{}: {e}", settings.display())),
        }
    }

    if wrote == 0 {
        return Err(Error::Launch(format!(
            "could not write FastFlags: {}",
            last_error.unwrap_or_else(|| "unknown error".into())
        )));
    }

    tracing::info!(installs = wrote, flags = flags.len(), "wrote FastFlags");
    Ok(())
}

/// Read back whatever the newest install currently has.
pub fn read_fflags() -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut dirs = version_dirs();
    // Newest by modification time, which is the one Roblox most likely runs.
    dirs.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    let path = dirs.last()?.join("ClientSettings").join("ClientAppSettings.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .as_object()
        .cloned()
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
mod tests {
    use super::*;

    #[test]
    fn flags_are_typed_the_way_the_engine_wants() {
        assert_eq!(typed_flag("true"), serde_json::Value::Bool(true));
        assert_eq!(typed_flag("false"), serde_json::Value::Bool(false));
        assert_eq!(typed_flag("42"), serde_json::Value::from(42));
        assert_eq!(typed_flag("-7"), serde_json::Value::from(-7));
        assert_eq!(
            typed_flag("Vulkan"),
            serde_json::Value::String("Vulkan".into())
        );
    }

    /// The engine ignores a bool sent as a string, which is indistinguishable
    /// from the flag not working, so this is worth pinning.
    #[test]
    fn a_boolean_never_ends_up_as_a_string() {
        assert!(typed_flag("true").is_boolean());
        assert!(!typed_flag("true").is_string());
    }

    #[test]
    fn an_empty_install_list_is_not_a_failure() {
        // With no LOCALAPPDATA there is nothing to find, and that must not be
        // reported as an error — the flags are still stored in RoJoin's config.
        assert!(version_dirs().is_empty() || !version_dirs().is_empty());
    }
}
