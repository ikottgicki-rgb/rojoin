//! Update checking.
//!
//! RoJoin currently has no public release feed — the repository is local with
//! no remote — so there is nothing to check against. Rather than pretend, the
//! check reports that honestly.
//!
//! When a feed exists, set `RELEASE_FEED` to a GitHub releases API URL and the
//! rest works: it compares the latest tag against the compiled-in version.

use serde::Deserialize;

/// A GitHub releases API URL, e.g.
/// `https://api.github.com/repos/<owner>/<repo>/releases/latest`.
/// `None` means updates are not configured for this build.
const RELEASE_FEED: Option<&str> = None;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
}

/// `Ok(Some(version))` when a newer release exists, `Ok(None)` when up to date.
pub async fn latest_version() -> rojoin_roblox::Result<Option<String>> {
    let Some(feed) = RELEASE_FEED else {
        return Err(rojoin_roblox::Error::Api(
            "Updates are not configured for this build.".into(),
        ));
    };

    let client = reqwest_client()?;
    let resp = client
        .get(feed)
        .header("user-agent", format!("RoJoin/{CURRENT}"))
        .send()
        .await
        .map_err(|e| rojoin_roblox::Error::Api(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(rojoin_roblox::Error::Api(format!(
            "release feed returned {}",
            resp.status()
        )));
    }

    let release: Release = resp
        .json()
        .await
        .map_err(|e| rojoin_roblox::Error::Api(e.to_string()))?;

    let latest = release.tag_name.trim_start_matches('v').to_string();
    Ok(is_newer(&latest, CURRENT).then_some(latest))
}

fn reqwest_client() -> rojoin_roblox::Result<reqwest::Client> {
    reqwest::Client::builder()
        .build()
        .map_err(|e| rojoin_roblox::Error::Api(e.to_string()))
}

/// Semantic-ish comparison. Compares numeric components left to right so
/// "0.10.0" correctly beats "0.9.0" — a plain string compare would not.
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split(['.', '-'])
            .map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>())
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };

    let a = parse(candidate);
    let b = parse(current);
    let len = a.len().max(b.len());

    for i in 0..len {
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
        // The case a string compare gets wrong.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn equal_versions_are_not_newer() {
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("1.2.3", "1.2.3"));
    }

    #[test]
    fn shorter_versions_pad_with_zero() {
        assert!(is_newer("1.1", "1.0.9"));
        assert!(!is_newer("1.0", "1.0.0"));
    }

    #[test]
    fn prerelease_suffixes_do_not_crash_the_parse() {
        assert!(is_newer("1.2.0-beta1", "1.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[tokio::test]
    async fn unconfigured_feed_reports_honestly_rather_than_claiming_up_to_date() {
        let err = latest_version().await.unwrap_err();
        assert!(err.to_string().contains("not configured"));
    }
}
