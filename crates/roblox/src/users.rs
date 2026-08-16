//! User lookups.
//!
//! `batch` is the one to reach for. Resolving names one at a time is what got
//! v2 throttled; the disk cache in `rojoin-store` sits on top of this so a name
//! is fetched once and then never again.

use crate::models::{AuthedUser, DataList, User};
use crate::{Client, Result};

const USERS: &str = "https://users.roblox.com/v1";

/// Who is this cookie? Also the liveness check for a stored session — a 401
/// here surfaces as `Error::Expired` and raises the sign-in banner.
pub async fn authenticated(client: &Client) -> Result<AuthedUser> {
    client.get_json(&format!("{USERS}/users/authenticated")).await
}

pub async fn get(client: &Client, user_id: i64) -> Result<User> {
    client.get_json(&format!("{USERS}/users/{user_id}")).await
}

/// Roblox caps this endpoint at 100 ids per request.
pub const BATCH: usize = 100;

/// Resolve many users.
///
/// **Partial-tolerant on purpose.** If a chunk fails — 429 is the common case —
/// this returns everything resolved so far instead of erroring. That matters
/// because the caller caches what it gets: throwing away 200 good names because
/// the third chunk was throttled means the next run re-requests all of them,
/// which throttles again. Returning partial results lets the cache converge.
pub async fn batch(client: &Client, user_ids: &[i64]) -> Result<Vec<User>> {
    let mut out = Vec::with_capacity(user_ids.len());

    for (i, chunk) in user_ids.chunks(BATCH).enumerate() {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        }

        let body = serde_json::json!({
            "userIds": chunk,
            "excludeBannedUsers": false,
        });

        match client.post_json::<DataList<User>>(&format!("{USERS}/users"), &body).await {
            Ok(list) => out.extend(list.data),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    resolved = out.len(),
                    remaining = user_ids.len() - out.len(),
                    "user batch interrupted; keeping what resolved"
                );
                break;
            }
        }
    }

    Ok(out)
}

/// Former usernames, shown as the "aka" line on a profile.
pub async fn previous_usernames(client: &Client, user_id: i64) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Name {
        name: String,
    }

    let url = format!("{USERS}/users/{user_id}/username-history?limit=25&sortOrder=Desc");
    let page: crate::models::Page<Name> = client.get_json(&url).await?;
    Ok(page.data.into_iter().map(|n| n.name).collect())
}

/// Account age in whole years, from the `created` timestamp.
pub fn account_age_years(created: &str) -> Option<i64> {
    let then = chrono::DateTime::parse_from_rfc3339(created).ok()?;
    let days = (chrono::Utc::now() - then.with_timezone(&chrono::Utc)).num_days();
    Some(days / 365)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_age_rejects_garbage_rather_than_panicking() {
        assert_eq!(account_age_years("not a date"), None);
        assert_eq!(account_age_years(""), None);
    }

    #[test]
    fn account_age_parses_roblox_timestamps() {
        let age = account_age_years("2006-02-27T21:06:40.3Z");
        assert!(age.is_some());
        assert!(age.unwrap() >= 19, "Roblox launched in 2006; got {age:?}");
    }

    #[test]
    fn batch_size_matches_roblox_limit() {
        assert_eq!(BATCH, 100);
    }

    #[test]
    fn user_deserializes_with_missing_optional_fields() {
        let u: User = serde_json::from_str(r#"{"id":1,"name":"a","displayName":"A"}"#).unwrap();
        assert_eq!(u.id, 1);
        assert!(u.description.is_none());
        assert!(!u.has_verified_badge);
    }
}
