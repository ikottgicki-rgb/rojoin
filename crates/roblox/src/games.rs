//! Game detail, servers, and sub-places.
//!
//! The universe/place distinction is the thing to keep straight, and getting it
//! wrong is silent rather than loud:
//!   * `games?universeIds=` takes **universe** ids.
//!   * `games/{id}/servers/Public` takes a **place** id. Handing it a universe
//!     id returns 400.
//!   * A universe contains many places. The one you launch by default is
//!     `rootPlaceId`; the others are the sub-places.

use crate::models::*;
use crate::{Client, Error, Result};

const GAMES: &str = "https://games.roblox.com/v1";
const GAMES_V2: &str = "https://games.roblox.com/v2";
const DEVELOP: &str = "https://develop.roblox.com/v1";
const BADGES: &str = "https://badges.roblox.com/v1";

/// The only `limit` values Roblox accepts on the servers endpoint. Anything
/// else comes back as `{"errors":[{"message":"Allowed values: 10, 25, 50, 100"}]}`.
pub const SERVER_LIMITS: [u32; 4] = [10, 25, 50, 100];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerSort {
    /// Roblox's default ordering.
    Default,
    /// Nearly-full first — best odds of landing with people.
    Fullest,
    /// Emptiest first — for when you want a quiet server.
    Emptiest,
}

/// Resolve a place id to the universe that owns it.
pub async fn universe_of(client: &Client, place_id: i64) -> Result<i64> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Resp {
        universe_id: Option<i64>,
    }

    let url = format!("https://apis.roblox.com/universes/v1/places/{place_id}/universe");
    let resp: Resp = client.get_json(&url).await?;
    resp.universe_id
        .ok_or_else(|| Error::Api(format!("place {place_id} belongs to no universe")))
}

pub async fn detail(client: &Client, universe_id: i64) -> Result<GameDetail> {
    let url = format!("{GAMES}/games?universeIds={universe_id}");
    let list: DataList<GameDetail> = client.get_json(&url).await?;
    list.data
        .into_iter()
        .next()
        .ok_or_else(|| Error::Api(format!("no game for universe {universe_id}")))
}

/// Batch detail lookup. Roblox caps this at 100 ids per call, so chunk.
pub async fn details(client: &Client, universe_ids: &[i64]) -> Result<Vec<GameDetail>> {
    let mut out = Vec::with_capacity(universe_ids.len());
    for chunk in universe_ids.chunks(100) {
        let ids = join_ids(chunk);
        let url = format!("{GAMES}/games?universeIds={ids}");
        let list: DataList<GameDetail> = client.get_json(&url).await?;
        out.extend(list.data);
    }
    Ok(out)
}

pub async fn votes(client: &Client, universe_ids: &[i64]) -> Result<Vec<Votes>> {
    let mut out = Vec::with_capacity(universe_ids.len());
    for chunk in universe_ids.chunks(100) {
        let ids = join_ids(chunk);
        let url = format!("{GAMES}/games/votes?universeIds={ids}");
        let list: DataList<Votes> = client.get_json(&url).await?;
        out.extend(list.data);
    }
    Ok(out)
}

/// Every place in a universe — this is the sub-place list.
///
/// Carried forward from v1-v3 as the feature users actually asked for: many
/// games put their real content in a sub-place, and launching the root place
/// dumps you in a lobby you then have to walk out of.
pub async fn places(client: &Client, universe_id: i64) -> Result<Vec<Place>> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut url = format!("{DEVELOP}/universes/{universe_id}/places?limit=100&sortOrder=Asc");
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={c}"));
        }

        let page: Page<Place> = client.get_json(&url).await?;
        out.extend(page.data);

        match page.next_page_cursor {
            Some(c) if !c.is_empty() => cursor = Some(c),
            _ => break,
        }
        if out.len() >= 500 {
            break;
        }
    }

    Ok(out)
}

/// Sub-places only: every place except the root one.
pub async fn sub_places(client: &Client, universe_id: i64, root_place_id: i64) -> Result<Vec<Place>> {
    Ok(places(client, universe_id)
        .await?
        .into_iter()
        .filter(|p| p.id != root_place_id)
        .collect())
}

/// Public servers for a **place** (not a universe).
pub async fn servers(
    client: &Client,
    place_id: i64,
    limit: u32,
    sort: ServerSort,
) -> Result<Vec<Server>> {
    let limit = nearest_allowed_limit(limit);
    let url = format!("{GAMES}/games/{place_id}/servers/Public?limit={limit}");
    let page: Page<Server> = client.get_json(&url).await?;

    let mut servers = page.data;
    match sort {
        ServerSort::Default => {}
        ServerSort::Fullest => servers.sort_by(|a, b| {
            free_slots(a).cmp(&free_slots(b)).then(b.playing.cmp(&a.playing))
        }),
        ServerSort::Emptiest => servers.sort_by(|a, b| a.playing.cmp(&b.playing)),
    }
    Ok(servers)
}

pub async fn media(client: &Client, universe_id: i64) -> Result<Vec<MediaItem>> {
    let url = format!("{GAMES_V2}/games/{universe_id}/media");
    let list: DataList<MediaItem> = client.get_json(&url).await?;
    Ok(list.data)
}

pub async fn badges(client: &Client, universe_id: i64) -> Result<Vec<Badge>> {
    let url = format!("{BADGES}/universes/{universe_id}/badges?limit=100&sortOrder=Asc");
    let page: Page<Badge> = client.get_json(&url).await?;
    Ok(page.data)
}

pub async fn game_passes(client: &Client, universe_id: i64) -> Result<Vec<GamePass>> {
    let url = format!("{GAMES}/games/{universe_id}/game-passes?limit=100&sortOrder=Asc");
    let page: Page<GamePass> = client.get_json(&url).await?;
    Ok(page.data)
}

/// Games a user has favourited. Public, so it works for other people's
/// profiles as well as your own.
pub async fn user_favorites(client: &Client, user_id: i64, limit: u32) -> Result<Vec<GameDetail>> {
    let url = format!("{GAMES_V2}/users/{user_id}/favorite/games?limit={limit}&sortOrder=Asc");
    let page: Page<GameDetail> = client.get_json(&url).await?;
    Ok(page.data)
}

/// Games published by a group.
pub async fn group_games(client: &Client, group_id: i64, limit: u32) -> Result<Vec<GameDetail>> {
    let url = format!("{GAMES_V2}/groups/{group_id}/games?limit={limit}&sortOrder=Asc");
    let page: Page<GameDetail> = client.get_json(&url).await?;
    Ok(page.data)
}

pub async fn is_favorited(client: &Client, universe_id: i64) -> Result<bool> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Resp {
        is_favorited: bool,
    }
    let url = format!("{GAMES}/games/{universe_id}/favorites");
    let resp: Resp = client.get_json(&url).await?;
    Ok(resp.is_favorited)
}

pub async fn set_favorited(client: &Client, universe_id: i64, favorited: bool) -> Result<()> {
    let url = format!("{GAMES}/games/{universe_id}/favorites");
    let body = serde_json::json!({ "isFavorited": favorited });
    let _: serde_json::Value = client.post_json(&url, &body).await?;
    Ok(())
}

fn join_ids(ids: &[i64]) -> String {
    ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

fn free_slots(s: &Server) -> i32 {
    (s.max_players - s.playing).max(0)
}

/// Snap to the nearest allowed value rather than erroring. A caller asking for
/// 20 servers wants "about 20", not a 400.
fn nearest_allowed_limit(requested: u32) -> u32 {
    SERVER_LIMITS
        .iter()
        .copied()
        .min_by_key(|l| l.abs_diff(requested))
        .unwrap_or(25)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_limits_snap_to_allowed_values() {
        assert_eq!(nearest_allowed_limit(1), 10);
        assert_eq!(nearest_allowed_limit(20), 25);
        assert_eq!(nearest_allowed_limit(30), 25);
        assert_eq!(nearest_allowed_limit(60), 50);
        assert_eq!(nearest_allowed_limit(1000), 100);
    }

    #[test]
    fn fullest_sort_prefers_nearly_full_servers() {
        let mk = |playing, max| Server {
            id: format!("{playing}"),
            max_players: max,
            playing,
            ping: None,
            fps: None,
            player_tokens: None,
        };
        let mut v = vec![mk(2, 30), mk(29, 30), mk(15, 30)];
        v.sort_by(|a, b| free_slots(a).cmp(&free_slots(b)).then(b.playing.cmp(&a.playing)));
        assert_eq!(v.iter().map(|s| s.playing).collect::<Vec<_>>(), vec![29, 15, 2]);
    }

    #[test]
    fn subplace_filter_drops_only_the_root() {
        let all = vec![
            Place { id: 1, universe_id: 9, name: "Lobby".into(), description: None },
            Place { id: 2, universe_id: 9, name: "Arena".into(), description: None },
            Place { id: 3, universe_id: 9, name: "Shop".into(), description: None },
        ];
        let subs: Vec<_> = all.into_iter().filter(|p| p.id != 1).collect();
        assert_eq!(subs.len(), 2);
        assert!(subs.iter().all(|p| p.id != 1));
    }

    #[test]
    fn id_joining_matches_roblox_query_format() {
        assert_eq!(join_ids(&[1, 2, 3]), "1,2,3");
        assert_eq!(join_ids(&[]), "");
    }
}
