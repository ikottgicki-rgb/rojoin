//! Cookie storage.
//!
//! `.ROBLOSECURITY` is the whole account. It never goes in config.json — it
//! lives in the OS keyring (Secret Service on Linux, Credential Manager on
//! Windows), keyed by account id so multi-account works.
//!
//! There is a file fallback for machines with no working keyring (a headless
//! session, a container, a desktop without a Secret Service provider). It is
//! chmod 600 and clearly *worse*, so it warns loudly — but silently failing to
//! save the login and making the user sign in on every launch is worse still.

use std::path::PathBuf;

const SERVICE: &str = "se.sphinxly.rojoin-v4";

pub fn store(account_id: &str, cookie: &str) -> anyhow::Result<()> {
    match keyring::Entry::new(SERVICE, account_id) {
        Ok(entry) => match entry.set_password(cookie) {
            Ok(()) => {
                // A previous fallback file must not linger with a stale cookie.
                let _ = std::fs::remove_file(fallback_path(account_id));
                return Ok(());
            }
            Err(e) => tracing::warn!(error = %e, "keyring write failed, using file fallback"),
        },
        Err(e) => tracing::warn!(error = %e, "keyring unavailable, using file fallback"),
    }

    write_fallback(account_id, cookie)
}

pub fn load(account_id: &str) -> Option<String> {
    if let Ok(entry) = keyring::Entry::new(SERVICE, account_id) {
        if let Ok(cookie) = entry.get_password() {
            if !cookie.is_empty() {
                return Some(cookie);
            }
        }
    }

    std::fs::read_to_string(fallback_path(account_id))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn delete(account_id: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, account_id) {
        let _ = entry.delete_credential();
    }
    let _ = std::fs::remove_file(fallback_path(account_id));
}

fn fallback_path(account_id: &str) -> PathBuf {
    // Account ids come from Roblox and are numeric, but sanitise anyway rather
    // than trusting remote data to be path-safe.
    let safe: String = account_id.chars().filter(char::is_ascii_alphanumeric).collect();
    rojoin_store::config_dir().join(format!("session-{safe}"))
}

fn write_fallback(account_id: &str, cookie: &str) -> anyhow::Result<()> {
    let path = fallback_path(account_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, cookie)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_path_strips_anything_path_shaped() {
        // Roblox ids are numeric, but a malformed or hostile id must not be
        // able to escape the config directory.
        let p = fallback_path("../../etc/passwd");
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name, "session-etcpasswd");
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
    }

    #[test]
    fn normal_ids_pass_through() {
        let p = fallback_path("1234567890");
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "session-1234567890");
    }
}
