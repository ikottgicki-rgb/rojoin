//! Friends, presence, and the social write endpoints.
//!
//! Two shape gotchas, both verified against the live API:
//!   * `/friends/find` answers in **PascalCase** (`PageItems`, `NextCursor`)
//!     while the rest of the API is camelCase.
//!   * `/friends` (the flat list) caps out around 200, so it cannot be the
//!     primary source for anyone with a large friends list.
//!
//! The important behavioural rule: **never treat a partially-fetched list as
//! complete.** If pagination is interrupted — a 429 mid-walk is routine — the
//! partial result must be marked incomplete, or a throttle blip gets cached as
//! "these are all your friends" and people silently disappear from the UI.

use serde::Deserialize;

use crate::models::{DataList, User};
use crate::{Client, Result};

const FRIENDS: &str = "https://friends.roblox.com/v1";
const PRESENCE: &str = "https://presence.roblox.com/v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FindPage {
    #[serde(default = "Vec::new")]
    page_items: Vec<FindItem>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FindItem {
    /// Lowercase `id` even inside the PascalCase envelope.
    id: i64,
}

/// A friends list plus whether we actually managed to fetch all of it.
#[derive(Debug, Default)]
pub struct FriendList {
    pub ids: Vec<i64>,
    /// False when pagination was cut short. Callers must not cache an
    /// incomplete list as authoritative.
    pub complete: bool,
}

/// Every friend id, via the paginated endpoint, with the flat list unioned in
/// as a backfill so a throttled page still leaves us with something usable.
pub async fn friend_ids(client: &Client, user_id: i64) -> Result<FriendList> {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let mut complete = true;

    loop {
        let mut url = format!("{FRIENDS}/users/{user_id}/friends/find?limit=50");
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={c}"));
        }

        match client.get_json::<FindPage>(&url).await {
            Ok(page) => {
                ids.extend(page.page_items.into_iter().map(|i| i.id));
                match page.next_cursor {
                    Some(c) if !c.is_empty() => cursor = Some(c),
                    _ => break,
                }
            }
            Err(e) => {
                // Partial, and flagged as such.
                tracing::warn!(error = %e, "friends pagination interrupted");
                complete = false;
                break;
            }
        }

        if ids.len() >= 5_000 {
            complete = false;
            break;
        }
    }

    // Union the flat list. It caps around 200 but costs one request and heals
    // the common case where the first paginated page was the one that failed.
    if let Ok(flat) = client
        .get_json::<DataList<User>>(&format!("{FRIENDS}/users/{user_id}/friends"))
        .await
    {
        for u in flat.data {
            if !ids.contains(&u.id) {
                ids.push(u.id);
            }
        }
    }

    Ok(FriendList { ids, complete })
}

// --- presence ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceKind {
    Offline,
    Online,
    InGame,
    InStudio,
    Invisible,
}

impl PresenceKind {
    fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Online,
            2 => Self::InGame,
            3 => Self::InStudio,
            4 => Self::Invisible,
            _ => Self::Offline,
        }
    }

    /// Sort weight: in-game first, then online, then everyone else. Drives the
    /// friends-list ordering, where "who can I join right now" is the question
    /// being asked.
    pub fn rank(self) -> u8 {
        match self {
            Self::InGame => 0,
            Self::Online => 1,
            Self::InStudio => 2,
            Self::Invisible | Self::Offline => 3,
        }
    }

    pub fn is_joinable(self) -> bool {
        matches!(self, Self::InGame)
    }
}

#[derive(Debug, Clone)]
pub struct Presence {
    pub user_id: i64,
    pub kind: PresenceKind,
    pub location: String,
    pub place_id: Option<i64>,
    pub root_place_id: Option<i64>,
    pub game_id: Option<String>,
    pub universe_id: Option<i64>,
    pub last_online: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresenceResponse {
    #[serde(default = "Vec::new")]
    user_presences: Vec<RawPresence>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RawPresence {
    user_presence_type: i32,
    last_location: Option<String>,
    place_id: Option<i64>,
    root_place_id: Option<i64>,
    game_id: Option<String>,
    universe_id: Option<i64>,
    user_id: i64,
    last_online: Option<String>,
}

impl Default for RawPresence {
    fn default() -> Self {
        Self {
            user_presence_type: 0,
            last_location: None,
            place_id: None,
            root_place_id: None,
            game_id: None,
            universe_id: None,
            user_id: 0,
            last_online: None,
        }
    }
}

/// Presence for a batch of users. Roblox caps this well below the friend-list
/// size, so chunk it.
pub async fn presence(client: &Client, user_ids: &[i64]) -> Result<Vec<Presence>> {
    let mut out = Vec::with_capacity(user_ids.len());

    for chunk in user_ids.chunks(100) {
        let body = serde_json::json!({ "userIds": chunk });
        let resp: PresenceResponse = client
            .post_json(&format!("{PRESENCE}/presence/users"), &body)
            .await?;

        out.extend(resp.user_presences.into_iter().map(|p| Presence {
            user_id: p.user_id,
            kind: PresenceKind::from_code(p.user_presence_type),
            location: p.last_location.unwrap_or_default(),
            place_id: p.place_id,
            root_place_id: p.root_place_id,
            game_id: p.game_id,
            universe_id: p.universe_id,
            last_online: p.last_online,
        }));
    }

    Ok(out)
}

// --- requests ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct SocialCounts {
    pub friends: i64,
    pub followers: i64,
    pub following: i64,
}

/// The three counts shown on a profile. Each is fetched independently and a
/// failure degrades that one number to zero rather than blanking the profile.
pub async fn counts(client: &Client, user_id: i64) -> SocialCounts {
    async fn one(client: &Client, url: String) -> i64 {
        #[derive(Deserialize)]
        struct Count {
            count: i64,
        }
        client.get_json::<Count>(&url).await.map(|c| c.count).unwrap_or(0)
    }

    SocialCounts {
        friends: one(client, format!("{FRIENDS}/users/{user_id}/friends/count")).await,
        followers: one(client, format!("{FRIENDS}/users/{user_id}/followers/count")).await,
        following: one(client, format!("{FRIENDS}/users/{user_id}/followings/count")).await,
    }
}

pub async fn request_count(client: &Client) -> Result<i64> {
    #[derive(Deserialize)]
    struct Count {
        count: i64,
    }
    let c: Count = client.get_json(&format!("{FRIENDS}/user/friend-requests/count")).await?;
    Ok(c.count)
}

pub async fn requests(client: &Client, limit: u32) -> Result<Vec<User>> {
    let url = format!("{FRIENDS}/my/friends/requests?limit={limit}&sortOrder=Desc");
    let page: crate::models::Page<User> = client.get_json(&url).await?;
    Ok(page.data)
}

// --- writes -----------------------------------------------------------------

pub async fn send_request(client: &Client, user_id: i64) -> Result<()> {
    client.post_action(&format!("{FRIENDS}/users/{user_id}/request-friendship")).await
}

pub async fn accept(client: &Client, user_id: i64) -> Result<()> {
    client.post_action(&format!("{FRIENDS}/users/{user_id}/accept-friend-request")).await
}

pub async fn decline(client: &Client, user_id: i64) -> Result<()> {
    client.post_action(&format!("{FRIENDS}/users/{user_id}/decline-friend-request")).await
}

pub async fn unfriend(client: &Client, user_id: i64) -> Result<()> {
    client.post_action(&format!("{FRIENDS}/users/{user_id}/unfriend")).await
}

pub async fn follow(client: &Client, user_id: i64) -> Result<()> {
    client.post_action(&format!("{FRIENDS}/users/{user_id}/follow")).await
}

pub async fn unfollow(client: &Client, user_id: i64) -> Result<()> {
    client.post_action(&format!("{FRIENDS}/users/{user_id}/unfollow")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_page_parses_pascal_case() {
        // The one endpoint that breaks the API's camelCase convention.
        let json = r#"{"PreviousCursor":null,"PageItems":[{"id":1},{"id":2}],"NextCursor":"abc","HasMore":null}"#;
        let page: FindPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.page_items.len(), 2);
        assert_eq!(page.next_cursor.as_deref(), Some("abc"));
    }

    #[test]
    fn empty_find_page_is_not_an_error() {
        let json = r#"{"PreviousCursor":null,"PageItems":[],"NextCursor":null,"HasMore":null}"#;
        let page: FindPage = serde_json::from_str(json).unwrap();
        assert!(page.page_items.is_empty());
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn presence_codes_map_correctly() {
        assert_eq!(PresenceKind::from_code(0), PresenceKind::Offline);
        assert_eq!(PresenceKind::from_code(1), PresenceKind::Online);
        assert_eq!(PresenceKind::from_code(2), PresenceKind::InGame);
        assert_eq!(PresenceKind::from_code(3), PresenceKind::InStudio);
        assert_eq!(PresenceKind::from_code(4), PresenceKind::Invisible);
        // An unknown future code must degrade to Offline, not panic.
        assert_eq!(PresenceKind::from_code(99), PresenceKind::Offline);
    }

    #[test]
    fn only_in_game_is_joinable() {
        assert!(PresenceKind::InGame.is_joinable());
        for k in [
            PresenceKind::Online,
            PresenceKind::Offline,
            PresenceKind::InStudio,
            PresenceKind::Invisible,
        ] {
            assert!(!k.is_joinable(), "{k:?} must not offer a Join button");
        }
    }

    #[test]
    fn presence_ranking_puts_joinable_friends_first() {
        let mut kinds = vec![
            PresenceKind::Offline,
            PresenceKind::InGame,
            PresenceKind::InStudio,
            PresenceKind::Online,
        ];
        kinds.sort_by_key(|k| k.rank());
        assert_eq!(kinds[0], PresenceKind::InGame);
        assert_eq!(kinds[1], PresenceKind::Online);
    }

    #[test]
    fn presence_response_tolerates_nulls() {
        // Every optional field null is the normal offline-user response.
        let json = r#"{"userPresences":[{"userPresenceType":0,"lastLocation":null,
            "placeId":null,"rootPlaceId":null,"gameId":null,"universeId":null,"userId":156}]}"#;
        let resp: PresenceResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.user_presences.len(), 1);
        assert_eq!(resp.user_presences[0].user_id, 156);
    }
}
