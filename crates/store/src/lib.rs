//! On-disk state: config, per-account data, caches.
//!
//! Fresh schema at `~/.config/rojoin-v4` — deliberately NOT shared with v2/v3,
//! so every version can coexist without corrupting another's store.
//!
//! Two rules this module exists to enforce:
//!   * Everything user-scoped lives under `accounts[id]`, never at the top
//!     level. Global history/pins in v1 leaked between accounts and took an
//!     audit to unpick.
//!   * Writes are atomic (temp file + rename). A half-written config is worse
//!     than no config.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod playtime;
pub mod config;

pub use config::{Account, AccountData, Config, GameHistory, Settings};

/// Never hardcode this elsewhere. Tests must override `XDG_CONFIG_HOME`
/// *before* the first call — a v1 test once wrote into the user's real config.
pub fn config_dir() -> PathBuf {
    directories::ProjectDirs::from("se", "sphinxly", "rojoin-v4")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rojoin-v4"))
}

pub fn cache_dir() -> PathBuf {
    directories::ProjectDirs::from("se", "sphinxly", "rojoin-v4")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rojoin-v4-cache"))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Write a JSON file atomically: serialise to `<path>.tmp`, fsync, rename.
/// Rename is atomic on both Linux and Windows for same-directory targets.
pub fn write_atomic<T: Serialize>(path: &std::path::Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value)?;

    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }

    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &std::path::Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Persisted username/avatar lookups, keyed by user id.
///
/// This is the single most important cache in the app. v2 proved that resolving
/// names on demand gets the *account* rate-limited by Roblox, which then shows
/// up as friends rendering as "User 12345" and, worse, as friends silently
/// disappearing when a throttled page gets cached as a complete list.
/// Resolve once, keep it, and the throttle source is gone.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct NameCache {
    pub users: HashMap<String, CachedUser>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedUser {
    pub username: String,
    pub display_name: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub verified: bool,
    /// Unix seconds. Entries older than a week are refreshed opportunistically,
    /// never eagerly.
    pub ts: i64,
}

impl NameCache {
    pub fn path() -> PathBuf {
        config_dir().join("names.json")
    }

    pub fn load() -> Self {
        read_json(&Self::path())
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&Self::path(), self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_round_trips_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("rojoin-store-test-{}", std::process::id()));
        let path = dir.join("cfg.json");

        let mut cache = NameCache::default();
        cache.users.insert(
            "1".into(),
            CachedUser {
                username: "roblox".into(),
                display_name: "Roblox".into(),
                avatar_url: String::new(),
                verified: true,
                ts: 0,
            },
        );

        write_atomic(&path, &cache).unwrap();
        let back: NameCache = read_json(&path);
        assert_eq!(back.users["1"].username, "roblox");
        assert!(!path.with_extension("tmp").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_yields_default_not_panic() {
        let back: NameCache = read_json(std::path::Path::new("/nonexistent/nope.json"));
        assert!(back.users.is_empty());
    }
}
