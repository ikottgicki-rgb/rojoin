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
    let limit = crate::page_limit(limit);
    let url = format!("{USERS}/users/search?keyword={}&limit={limit}", urlencode(query));
    let page: Page<UserSearchResult> = client.get_json(&url).await?;
    Ok(page.data)
}

pub async fn groups(client: &Client, query: &str, limit: u32) -> Result<Vec<GroupSearchResult>> {
    let limit = crate::page_limit(limit);
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

/// A pasted link, broken into everything needed to join it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JoinTarget {
    pub place_id: i64,
    /// A specific running server, from `gameInstanceId`.
    pub job_id: Option<String>,
    /// A reserved server, from `accessCode`.
    pub access_code: Option<String>,
    /// A private/VIP server share link, from `privateServerLinkCode`.
    ///
    /// Not the same thing as `access_code`: Roblox joins them through different
    /// parameters, so they are carried separately all the way to the launcher.
    pub link_code: Option<String>,
}

/// Pull a query parameter out of a link without a URL crate.
///
/// Matches on `name=` preceded by `?` or `&` so `placeId` cannot be found
/// inside `rootPlaceId`, and stops at the next separator or fragment.
fn query_param(url: &str, name: &str) -> Option<String> {
    for sep in ['?', '&'] {
        let needle = format!("{sep}{name}=");
        if let Some(rest) = url.split(&needle).nth(1) {
            let value: String = rest
                .chars()
                .take_while(|c| *c != '&' && *c != '#')
                .collect();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Recognise a pasted link, including one that names a specific server.
///
/// Handles the game page, `games/start`, the `roblox://` deep link, a VIP
/// server link, and a bare place id. Anything else is a search term.
pub fn resolve_join_target(input: &str) -> Option<JoinTarget> {
    let place_id = resolve_place_id(input)?;
    let s = input.trim();

    Some(JoinTarget {
        place_id,
        job_id: query_param(s, "gameInstanceId").filter(|v| v.len() > 8),
        access_code: query_param(s, "accessCode").filter(|v| !v.is_empty()),
        link_code: query_param(s, "privateServerLinkCode")
            .or_else(|| query_param(s, "linkCode"))
            .filter(|v| !v.is_empty()),
    })
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
    fn a_server_link_carries_the_instance_through() {
        let t = resolve_join_target(
            "https://www.roblox.com/games/start?placeId=606849621&gameInstanceId=1a2b3c4d-5e6f-7890-abcd-ef1234567890",
        )
        .unwrap();
        assert_eq!(t.place_id, 606_849_621);
        assert_eq!(t.job_id.as_deref(), Some("1a2b3c4d-5e6f-7890-abcd-ef1234567890"));
        assert!(t.access_code.is_none() && t.link_code.is_none());
    }

    #[test]
    fn a_vip_link_carries_its_code() {
        let t = resolve_join_target(
            "https://www.roblox.com/games/606849621/Jailbreak?privateServerLinkCode=abc123XYZ",
        )
        .unwrap();
        assert_eq!(t.place_id, 606_849_621);
        assert_eq!(t.link_code.as_deref(), Some("abc123XYZ"));
        assert!(
            t.access_code.is_none(),
            "a share link is a link code, not an access code"
        );
    }

    #[test]
    fn a_plain_game_link_names_no_server() {
        let t = resolve_join_target("https://www.roblox.com/games/606849621/Jailbreak").unwrap();
        assert_eq!(t.place_id, 606_849_621);
        assert!(t.job_id.is_none() && t.access_code.is_none() && t.link_code.is_none());
    }

    #[test]
    fn a_deep_link_works_the_same_way() {
        let t = resolve_join_target(
            "roblox://experiences/start?placeId=920587237&gameInstanceId=deadbeef-0000-1111-2222-333344445555",
        )
        .unwrap();
        assert_eq!(t.place_id, 920_587_237);
        assert!(t.job_id.is_some());
    }

    #[test]
    fn a_param_is_not_found_inside_a_longer_name() {
        // `rootPlaceId=` must not satisfy a search for `placeId=`.
        assert_eq!(query_param("https://x/y?rootPlaceId=5", "placeId"), None);
        assert_eq!(query_param("https://x/y?placeId=5", "placeId").as_deref(), Some("5"));
    }

    #[test]
    fn a_truncated_instance_id_is_rejected_rather_than_joined() {
        // A stray short value is far more likely junk than a real JobId.
        let t = resolve_join_target("https://www.roblox.com/games/start?placeId=1&gameInstanceId=x")
            .unwrap();
        assert!(t.job_id.is_none());
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

#[cfg(test)]
mod limit_tests {
    use crate::page_limit;

    #[test]
    fn only_ever_asks_for_a_size_roblox_accepts() {
        for requested in 0..=200u32 {
            assert!(
                matches!(page_limit(requested), 10 | 25 | 50 | 100),
                "{requested} produced a limit Roblox rejects"
            );
        }
    }

    #[test]
    fn rounds_up_so_a_request_is_never_short_changed() {
        assert_eq!(page_limit(8), 10);
        assert_eq!(page_limit(12), 25);
        assert_eq!(page_limit(50), 50);
        assert_eq!(page_limit(500), 100);
    }
}
