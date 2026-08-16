//! Search across games, users and groups.
//!
//! Games go through `search-api/omni-search`. The old
//! `games.roblox.com/v1/games/list` endpoint is **gone** — it answers 404 —
//! so do not reach for it.
//!
//! omni-search nests results by content type:
//! `{ searchResults: [ { contentGroupType: "Game", contents: [ ... ] } ] }`

use serde::Deserialize;

use crate::models::{GroupSearchResult, Page, UserSearchResult};
use crate::{Client, Result};

const OMNI: &str = "https://apis.roblox.com/search-api/omni-search";
const USERS: &str = "https://users.roblox.com/v1";
const GROUPS: &str = "https://groups.roblox.com/v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmniResponse {
    #[serde(default = "Vec::new")]
    search_results: Vec<OmniGroup>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmniGroup {
    #[serde(default)]
    content_group_type: String,
    #[serde(default = "Vec::new")]
    contents: Vec<OmniGame>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OmniGame {
    pub universe_id: i64,
    pub root_place_id: i64,
    pub name: String,
    pub description: String,
    pub player_count: i64,
    pub total_up_votes: i64,
    pub total_down_votes: i64,
    pub creator_name: String,
    pub creator_has_verified_badge: bool,
    pub is_sponsored: bool,
}

impl OmniGame {
    pub fn percent(&self) -> Option<i32> {
        let total = self.total_up_votes + self.total_down_votes;
        (total > 0)
            .then(|| ((self.total_up_votes as f64 / total as f64) * 100.0).round() as i32)
    }
}

pub struct GameSearchPage {
    pub games: Vec<OmniGame>,
    pub next: Option<String>,
}

/// Search games.
///
/// `session_id` should be stable for a given search session — Roblox uses it
/// for result continuity across pages.
pub async fn games(
    client: &Client,
    query: &str,
    session_id: &str,
    page_token: Option<&str>,
) -> Result<GameSearchPage> {
    let encoded = urlencode(query);
    let mut url = format!("{OMNI}?searchQuery={encoded}&sessionId={session_id}");
    url.push_str(&format!("&pageToken={}", page_token.unwrap_or("")));

    let resp: OmniResponse = client.get_json(&url).await?;

    let games = resp
        .search_results
        .into_iter()
        .filter(|g| g.content_group_type == "Game")
        .flat_map(|g| g.contents)
        .filter(|g| !g.is_sponsored)
        .collect();

    Ok(GameSearchPage { games, next: resp.next_page_token })
}

pub async fn users(client: &Client, query: &str, limit: u32) -> Result<Vec<UserSearchResult>> {
    let url = format!("{USERS}/users/search?keyword={}&limit={limit}", urlencode(query));
    let page: Page<UserSearchResult> = client.get_json(&url).await?;
    Ok(page.data)
}

pub async fn groups(client: &Client, query: &str, limit: u32) -> Result<Vec<GroupSearchResult>> {
    let url = format!(
        "{GROUPS}/groups/search?keyword={}&limit={limit}&prioritizeExactMatch=true",
        urlencode(query)
    );
    let page: Page<GroupSearchResult> = client.get_json(&url).await?;
    Ok(page.data)
}

/// Minimal percent-encoding for query strings. Pulling in a crate for this
/// would be the only reason to have one.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Recognise a pasted Roblox link or a bare place id.
///
/// The 8-digit floor matters: a game genuinely *named* with short digits (say
/// "99") must still search rather than silently jumping to place 99.
pub fn resolve_place_id(input: &str) -> Option<i64> {
    let s = input.trim();

    if let Some(rest) = s.split("/games/").nth(1) {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = digits.parse::<i64>() {
            return Some(id);
        }
    }

    if let Some(rest) = s.split("placeId=").nth(1) {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(id) = digits.parse::<i64>() {
            return Some(id);
        }
    }

    if s.len() >= 8 && s.chars().all(|c| c.is_ascii_digit()) {
        return s.parse().ok();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_spaces_and_symbols() {
        assert_eq!(urlencode("tower defense"), "tower%20defense");
        assert_eq!(urlencode("a&b=c"), "a%26b%3Dc");
        assert_eq!(urlencode("plain-Text_1.0~"), "plain-Text_1.0~");
    }

    #[test]
    fn resolves_game_links() {
        assert_eq!(
            resolve_place_id("https://www.roblox.com/games/606849621/Jailbreak"),
            Some(606849621)
        );
        assert_eq!(
            resolve_place_id("https://www.roblox.com/games/start?placeId=606849621"),
            Some(606849621)
        );
    }

    #[test]
    fn bare_long_id_resolves_but_short_digits_stay_a_search() {
        assert_eq!(resolve_place_id("606849621"), Some(606849621));
        assert_eq!(resolve_place_id("99"), None);
        assert_eq!(resolve_place_id("1234567"), None, "7 digits is below the floor");
    }

    #[test]
    fn plain_words_are_never_place_ids() {
        assert_eq!(resolve_place_id("jailbreak"), None);
        assert_eq!(resolve_place_id(""), None);
    }

    #[test]
    fn omni_percent_handles_unvoted() {
        let g = OmniGame { total_up_votes: 0, total_down_votes: 0, ..Default::default() };
        assert_eq!(g.percent(), None);

        let g = OmniGame { total_up_votes: 3, total_down_votes: 1, ..Default::default() };
        assert_eq!(g.percent(), Some(75));
    }

    #[test]
    fn omni_parsing_skips_non_game_groups_and_sponsored() {
        let json = r#"{
            "searchResults": [
                {"contentGroupType":"Game","contents":[
                    {"universeId":1,"name":"Real","isSponsored":false},
                    {"universeId":2,"name":"Ad","isSponsored":true}
                ]},
                {"contentGroupType":"Creator","contents":[]}
            ],
            "nextPageToken":"tok"
        }"#;

        let resp: OmniResponse = serde_json::from_str(json).unwrap();
        let games: Vec<_> = resp
            .search_results
            .into_iter()
            .filter(|g| g.content_group_type == "Game")
            .flat_map(|g| g.contents)
            .filter(|g| !g.is_sponsored)
            .collect();

        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "Real");
    }
}
