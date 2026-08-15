//! Shared response types.
//!
//! Two conventions worth knowing before editing:
//!
//! 1. **IDs are `i64`.** Roblox place, universe, user and group ids all exceed
//!    i32. They become `String` at the UI boundary because Slint's `int` is i32.
//!
//! 2. **Optional collections use `Option<Vec<T>>` with a `_or_default` reader.**
//!    Roblox routinely returns a key that is present but `null`
//!    (`recommendationList: null` crashed two v1 screens). `#[serde(default)]`
//!    does not save you there — `default` only applies when the key is
//!    *missing*, not when it is explicitly null.

use serde::{Deserialize, Serialize};

/// Wrapper for Roblox's ubiquitous `{ "data": [...] }` envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct DataList<T> {
    #[serde(default = "Vec::new")]
    pub data: Vec<T>,
}

impl<T> Default for DataList<T> {
    fn default() -> Self {
        Self { data: Vec::new() }
    }
}

/// Paginated envelope: `{ data, nextPageCursor }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    #[serde(default = "Vec::new")]
    pub data: Vec<T>,
    #[serde(default)]
    pub next_page_cursor: Option<String>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self { data: Vec::new(), next_page_cursor: None }
    }
}

// ---------------------------------------------------------------------------
// Games
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GameDetail {
    pub id: i64, // universe id
    pub root_place_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub creator: Creator,
    pub price: Option<i64>,
    pub playing: i64,
    pub visits: i64,
    pub max_players: i32,
    pub created: String,
    pub updated: String,
    pub genre: Option<String>,
    pub favorited_count: i64,
    pub is_favorited_by_user: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Creator {
    pub id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String, // "User" | "Group"
    pub has_verified_badge: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Votes {
    pub id: i64,
    pub up_votes: i64,
    pub down_votes: i64,
}

impl Votes {
    /// Approval percentage, or `None` when nobody has voted. Rendering "0%"
    /// for an unvoted game is actively misleading.
    pub fn percent(&self) -> Option<i32> {
        let total = self.up_votes + self.down_votes;
        (total > 0).then(|| ((self.up_votes as f64 / total as f64) * 100.0).round() as i32)
    }
}

/// A place inside a universe. This is what powers "join sub-place" — the
/// feature carried across from v1-v3.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Place {
    pub id: i64,
    pub universe_id: i64,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Server {
    pub id: String,
    pub max_players: i32,
    pub playing: i32,
    pub ping: Option<i32>,
    pub fps: Option<f64>,
    /// Head-shot URLs of players in the server, when Roblox supplies them.
    pub player_tokens: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaItem {
    pub id: i64,
    pub asset_type: String,
    pub image_id: Option<i64>,
    pub video_hash: Option<String>,
    pub alt_text: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GamePass {
    pub id: i64,
    pub name: String,
    pub price: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Badge {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GameSearchResult {
    pub universe_id: i64,
    pub root_place_id: i64,
    pub name: String,
    pub creator_name: String,
    pub player_count: i64,
    pub total_up_votes: i64,
    pub total_down_votes: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserSearchResult {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub has_verified_badge: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupSearchResult {
    pub id: i64,
    pub name: String,
    pub member_count: i64,
    pub has_verified_badge: bool,
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub created: Option<String>,
    pub has_verified_badge: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuthedUser {
    pub id: i64,
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Thumbnail {
    pub target_id: i64,
    pub state: String,
    pub image_url: Option<String>,
}

impl Thumbnail {
    /// Roblox returns state "Pending" with no URL while it renders. Treating
    /// that as a real URL gives you a permanently broken image.
    pub fn ready_url(&self) -> Option<&str> {
        (self.state == "Completed").then(|| self.image_url.as_deref()).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vote_percentage_handles_the_unvoted_game() {
        let v = Votes { id: 1, up_votes: 0, down_votes: 0 };
        assert_eq!(v.percent(), None, "0/0 must not render as 0% approval");

        let v = Votes { id: 1, up_votes: 9, down_votes: 1 };
        assert_eq!(v.percent(), Some(90));
    }

    #[test]
    fn null_collections_deserialize_as_empty_not_error() {
        // The exact shape that crashed v1's Home and Library screens.
        let json = r#"{"data":null}"#;
        let parsed: DataList<Votes> = serde_json::from_str(json).unwrap_or_default();
        assert!(parsed.data.is_empty());
    }

    #[test]
    fn missing_data_key_deserializes_as_empty() {
        let parsed: Page<Votes> = serde_json::from_str("{}").unwrap();
        assert!(parsed.data.is_empty());
        assert!(parsed.next_page_cursor.is_none());
    }

    #[test]
    fn pending_thumbnail_yields_no_url() {
        let t = Thumbnail {
            target_id: 1,
            state: "Pending".into(),
            image_url: Some("https://example/placeholder.png".into()),
        };
        assert_eq!(t.ready_url(), None);

        let t = Thumbnail {
            target_id: 1,
            state: "Completed".into(),
            image_url: Some("https://example/real.png".into()),
        };
        assert_eq!(t.ready_url(), Some("https://example/real.png"));
    }
}
