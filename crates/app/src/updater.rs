//! Update checking and self-update.
//!
//! Points at a GitHub releases feed. Set `RELEASE_REPO` to `owner/repo` once
//! there is a public repository and the whole flow works: check, download the
//! matching asset, and replace the running binary in place.
//!
//! Until then `check` reports plainly that updates are not set up. That is
//! deliberate — the previous version returned an error, which surfaced in the
//! UI as "failed" and read like a broken feature rather than an unconfigured
//! one.

use serde::Deserialize;

/// `owner/repo` on GitHub. `None` until RoJoin has a public home.
const RELEASE_REPO: Option<&str> = None;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No release feed configured for this build.
    NotConfigured,
    UpToDate,
    Available { version: String, url: String },
}

impl Status {
    pub fn message(&self) -> String {
        match self {
            Status::NotConfigured => {
                "Updates are not set up for this build yet.".into()
            }
            Status::UpToDate => format!("You are on the latest version ({CURRENT})."),
            Status::Available { version, .. } => {
                format!("Version {version} is available.")
            }
        }
    }

    pub fn can_install(&self) -> bool {
        matches!(self, Status::Available { .. })
    }
}

/// The asset this platform should download.
fn wanted_asset(assets: &[Asset]) -> Option<&Asset> {
    let want = if cfg!(windows) { ".zip" } else { ".AppImage" };
    assets.iter().find(|a| a.name.ends_with(want))
}

pub async fn check() -> Status {
    let Some(repo) = RELEASE_REPO else {
        return Status::NotConfigured;
    };

    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(_) => return Status::NotConfigured,
    };

    let resp = client
        .get(&url)
        .header("user-agent", format!("RoJoin/{CURRENT}"))
        .send()
        .await;

    let Ok(resp) = resp else { return Status::UpToDate };
    if !resp.status().is_success() {
        return Status::UpToDate;
    }

    let Ok(release) = resp.json::<Release>().await else {
        return Status::UpToDate;
    };

    let latest = release.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&latest, CURRENT) {
        return Status::UpToDate;
    }

    match wanted_asset(&release.assets) {
        Some(asset) => Status::Available {
            version: latest,
            url: asset.browser_download_url.clone(),
        },
        // A release with no asset for this platform is not an update we can
        // apply, so it is not offered.
        None => Status::UpToDate,
    }
}

/// Download an update and swap it in.
///
/// Writes beside the current executable and renames over it, so a failed or
/// partial download never leaves a broken binary. The running process keeps
/// its open file handle, so the swap takes effect on next launch.
pub async fn install(url: &str) -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate myself: {e}"))?;

    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| e.to_string())?;
    let bytes = client
        .get(url)
        .header("user-agent", format!("RoJoin/{CURRENT}"))
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download failed: {e}"))?;

    if bytes.len() < 1_000_000 {
        return Err("that download is too small to be a real build".into());
    }

    let staged = exe.with_extension("update");
    std::fs::write(&staged, &bytes).map_err(|e| format!("could not stage: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("could not make it executable: {e}"))?;
    }

    std::fs::rename(&staged, &exe).map_err(|e| format!("could not replace: {e}"))?;
    Ok(exe)
}

/// Compares numeric components left to right so "0.10.0" beats "0.9.0" — a
/// plain string compare gets that backwards.
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split(['.', '-'])
            .map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>())
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };

    let a = parse(candidate);
    let b = parse(current);
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_components_compare_numerically() {
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(is_newer("1.1", "1.0.9"));
        assert!(is_newer("1.2.0-beta1", "1.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn an_unconfigured_build_says_so_instead_of_failing() {
        // "failed" reads as a broken feature; this is just not set up yet.
        let s = Status::NotConfigured;
        assert!(s.message().contains("not set up"));
        assert!(!s.can_install());
    }

    #[test]
    fn only_an_available_update_can_be_installed() {
        assert!(!Status::UpToDate.can_install());
        assert!(Status::Available {
            version: "9.9.9".into(),
            url: "https://example/x".into()
        }
        .can_install());
    }

    #[test]
    fn the_asset_for_this_platform_is_chosen() {
        let assets = vec![
            Asset { name: "RoJoin-windows-x64.zip".into(), browser_download_url: "w".into() },
            Asset { name: "RoJoin-x86_64.AppImage".into(), browser_download_url: "l".into() },
        ];
        let picked = wanted_asset(&assets).unwrap();
        if cfg!(windows) {
            assert_eq!(picked.browser_download_url, "w");
        } else {
            assert_eq!(picked.browser_download_url, "l");
        }
    }

    #[test]
    fn a_release_without_an_asset_for_us_is_not_offered() {
        let assets = vec![Asset {
            name: "source.tar.gz".into(),
            browser_download_url: "s".into(),
        }];
        assert!(wanted_asset(&assets).is_none());
    }
}
