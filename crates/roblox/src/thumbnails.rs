//! Thumbnail URL resolution.
//!
//! Always batched. Thumbnails are the single highest-volume call the app makes
//! and the fastest way to get the user's account rate-limited — one request per
//! visible tile is how v2 ended up with friends rendering as "User 12345".
//!
//! Roblox returns `state: "Pending"` with a placeholder URL while it renders an
//! asset. `Thumbnail::ready_url` filters those out so we retry later rather
//! than caching a broken image forever.

use std::collections::HashMap;

use serde::Deserialize;

use crate::models::Thumbnail;
use crate::{Client, Result};

const THUMBS: &str = "https://thumbnails.roblox.com/v1";

/// Roblox caps batch thumbnail requests at 100 ids.
const BATCH: usize = 100;

#[derive(Debug, Deserialize)]
struct ThumbList {
    #[serde(default, deserialize_with = "crate::null_vec")]
    data: Vec<Thumbnail>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GameThumbGroup {
    universe_id: i64,
    #[serde(default, deserialize_with = "crate::null_vec")]
    thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Deserialize)]
struct GameThumbList {
    #[serde(default, deserialize_with = "crate::null_vec")]
    data: Vec<GameThumbGroup>,
}

/// Wide game art, keyed by universe id.
pub async fn game_art(client: &Client, universe_ids: &[i64]) -> Result<HashMap<i64, String>> {
    let mut out = HashMap::new();

    for chunk in universe_ids.chunks(BATCH) {
        let url = format!(
            "{THUMBS}/games/multiget/thumbnails?universeIds={}&size=768x432&format=Png&isCircular=false",
            join(chunk)
        );
        let list: GameThumbList = client.get_json(&url).await?;
        for group in list.data {
            if let Some(url) = group.thumbnails.iter().find_map(|t| t.ready_url()) {
                out.insert(group.universe_id, url.to_string());
            }
        }
    }

    Ok(out)
}

/// Square game icons, keyed by universe id.
pub async fn game_icons(client: &Client, universe_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, universe_ids, |ids| {
        format!("{THUMBS}/games/icons?universeIds={ids}&size=512x512&format=Png&isCircular=false")
    })
    .await
}

/// Circular head shots, keyed by user id.
pub async fn headshots(client: &Client, user_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, user_ids, |ids| {
        format!("{THUMBS}/users/avatar-headshot?userIds={ids}&size=150x150&format=Png&isCircular=false")
    })
    .await
}

/// Full-body avatar renders, keyed by user id.
pub async fn avatars(client: &Client, user_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, user_ids, |ids| {
        format!("{THUMBS}/users/avatar?userIds={ids}&size=420x420&format=Png&isCircular=false")
    })
    .await
}

/// An avatar render, waited on until Roblox has actually drawn it.
///
/// Changing what you are wearing invalidates the existing render, and the
/// thumbnail service answers `state: "Pending"` with no usable URL while it
/// redraws. A single fetch straight after a change therefore returns nothing,
/// which looks exactly like a view that never finishes loading. Poll instead.
pub async fn avatar_when_ready(client: &Client, user_id: i64) -> Option<String> {
    const WAITS_MS: [u64; 5] = [0, 700, 1500, 2500, 4000];

    for wait in WAITS_MS {
        if wait > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        }
        if let Ok(map) = avatars(client, &[user_id]).await {
            if let Some(url) = map.get(&user_id) {
                return Some(url.clone());
            }
        }
    }

    tracing::debug!(user_id, "avatar render still pending after waiting");
    None
}

pub async fn group_icons(client: &Client, group_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, group_ids, |ids| {
        format!("{THUMBS}/groups/icons?groupIds={ids}&size=420x420&format=Png&isCircular=false")
    })
    .await
}

/// Saved-outfit renders, keyed by outfit id.
pub async fn outfits(client: &Client, outfit_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, outfit_ids, |ids| {
        format!("{THUMBS}/users/outfits?userOutfitIds={ids}&size=150x150&format=Png&isCircular=false")
    })
    .await
}

/// Badge icons, keyed by badge id.
pub async fn badges(client: &Client, badge_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, badge_ids, |ids| {
        format!("{THUMBS}/badges/icons?badgeIds={ids}&size=150x150&format=Png&isCircular=false")
    })
    .await
}

/// Game-pass icons, keyed by pass id.
pub async fn game_passes(client: &Client, pass_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, pass_ids, |ids| {
        format!("{THUMBS}/game-passes?gamePassIds={ids}&size=150x150&format=Png&isCircular=false")
    })
    .await
}

pub async fn assets(client: &Client, asset_ids: &[i64]) -> Result<HashMap<i64, String>> {
    batch(client, asset_ids, |ids| {
        format!("{THUMBS}/assets?assetIds={ids}&size=420x420&format=Png&isCircular=false")
    })
    .await
}

/// Resolve head shots from server player *tokens*.
///
/// Servers do not expose user ids — only opaque tokens — so these cannot go
/// through the by-id endpoints. Results come back in request order.
pub async fn by_tokens(client: &Client, tokens: &[String]) -> Vec<Option<String>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Item {
        request_id: String,
        #[serde(default)]
        state: String,
        #[serde(default)]
        image_url: Option<String>,
    }
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default, deserialize_with = "crate::null_vec")]
        data: Vec<Item>,
    }

    let mut out = vec![None; tokens.len()];

    for (chunk_i, chunk) in tokens.chunks(BATCH).enumerate() {
        let body: Vec<serde_json::Value> = chunk
            .iter()
            .enumerate()
            .map(|(i, token)| {
                serde_json::json!({
                    "requestId": (chunk_i * BATCH + i).to_string(),
                    "type": "AvatarHeadShot",
                    "targetId": 0,
                    "token": token,
                    "format": "png",
                    "size": "150x150",
                })
            })
            .collect();

        let Ok(resp) = client
            .post_json::<Resp>("https://thumbnails.roblox.com/v1/batch", &serde_json::json!(body))
            .await
        else {
            break;
        };

        for item in resp.data {
            if item.state != "Completed" {
                continue;
            }
            if let Ok(idx) = item.request_id.parse::<usize>() {
                if let Some(slot) = out.get_mut(idx) {
                    *slot = item.image_url;
                }
            }
        }
    }

    out
}

async fn batch<F>(client: &Client, ids: &[i64], make_url: F) -> Result<HashMap<i64, String>>
where
    F: Fn(String) -> String,
{
    let mut out = HashMap::new();

    for chunk in ids.chunks(BATCH) {
        let url = make_url(join(chunk));
        let list: ThumbList = client.get_json(&url).await?;
        for t in list.data {
            if let Some(u) = t.ready_url() {
                out.insert(t.target_id, u.to_string());
            }
        }
    }

    Ok(out)
}

fn join(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_thumbnails_are_dropped_not_cached() {
        let json = r#"{"data":[
            {"targetId":1,"state":"Completed","imageUrl":"https://img/1.png"},
            {"targetId":2,"state":"Pending","imageUrl":"https://img/placeholder.png"},
            {"targetId":3,"state":"Blocked","imageUrl":null}
        ]}"#;

        let list: ThumbList = serde_json::from_str(json).unwrap();
        let ready: HashMap<i64, String> = list
            .data
            .iter()
            .filter_map(|t| t.ready_url().map(|u| (t.target_id, u.to_string())))
            .collect();

        assert_eq!(ready.len(), 1);
        assert_eq!(ready[&1], "https://img/1.png");
        assert!(!ready.contains_key(&2), "pending must not be cached as final");
    }

    #[test]
    fn game_art_picks_the_first_ready_thumbnail_per_universe() {
        let json = r#"{"data":[
            {"universeId":10,"thumbnails":[
                {"targetId":1,"state":"Pending","imageUrl":null},
                {"targetId":2,"state":"Completed","imageUrl":"https://img/real.png"}
            ]},
            {"universeId":11,"thumbnails":[]}
        ]}"#;

        let list: GameThumbList = serde_json::from_str(json).unwrap();
        let mut out = HashMap::new();
        for g in list.data {
            if let Some(u) = g.thumbnails.iter().find_map(|t| t.ready_url()) {
                out.insert(g.universe_id, u.to_string());
            }
        }

        assert_eq!(out[&10], "https://img/real.png");
        assert!(!out.contains_key(&11), "empty thumbnail list must yield nothing");
    }

    #[test]
    fn ids_join_in_roblox_query_format() {
        assert_eq!(join(&[1, 2, 3]), "1,2,3");
    }
}
