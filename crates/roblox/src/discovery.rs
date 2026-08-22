//! Roblox's own home feed (`discovery-api/omni-recommendation`).
//!
//! Used for exactly one thing: the **Continue** sort, which is Roblox's
//! recently-played list.
//!
//! Reading recency from Roblox instead of counting launches locally means the
//! list reflects everything the account actually played — a session on a phone,
//! a join from the website, a launch from the official client — not just what
//! went through RoJoin. There is also no local list left to drift out of sync,
//! and because the feed answers with universe ids the caller is spared a
//! place-to-universe lookup per row.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::{Client, Result};

const OMNI: &str = "https://apis.roblox.com/discovery-api/omni-recommendation";

/// The "Continue" sort.
///
/// Matched on the numeric id, never on the `topic` string: that string is
/// display text and comes back localised, so `topic == "Continue"` would work
/// in English and silently return nothing everywhere else.
const CONTINUE_TOPIC_ID: i64 = 100_000_003;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Feed {
    sorts: Vec<Sort>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Sort {
    topic_id: i64,
    recommendation_list: Vec<Recommendation>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Recommendation {
    content_type: String,
    content_id: i64,
}

/// Universe ids the account played most recently, newest first.
///
/// An empty result is a normal answer, not a failure — a brand-new account has
/// nothing to continue, and Roblox drops the sort entirely rather than sending
/// an empty one.
pub async fn recently_played(client: &Client, limit: usize) -> Result<Vec<i64>> {
    // This body is the one verified against the live endpoint. The treatment
    // list does not gate the Continue sort (it comes back as a `Carousel`
    // either way) but the feed is picky about being asked properly.
    let body = serde_json::json!({
        "pageType": "Home",
        "sessionId": session_id(),
        "supportedTreatmentTypes": [
            "SortlessGrid",
            "CarouselComponent",
            "AvatarCarousel",
            "FriendCarousel",
            "InterestTitle"
        ],
    });

    let feed: Feed = client.post_json(OMNI, &body).await?;
    Ok(continue_universes(&feed, limit))
}

fn continue_universes(feed: &Feed, limit: usize) -> Vec<i64> {
    let Some(sort) = feed.sorts.iter().find(|s| s.topic_id == CONTINUE_TOPIC_ID) else {
        tracing::info!("the home feed carried no Continue sort");
        return Vec::new();
    };

    sort.recommendation_list
        .iter()
        .filter(|r| r.content_type == "Game")
        .map(|r| r.content_id)
        .filter(|id| *id != 0)
        .take(limit)
        .collect()
}

/// A per-process session id for the feed.
///
/// Roblox groups a browsing session by this, so one value for the life of the
/// app is the honest answer. It only has to be GUID-shaped — it identifies a
/// session, not the user — so the clock is a good enough source and saves a
/// dependency.
fn session_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();

    ID.get_or_init(|| {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            (n & 0xffff_ffff) as u32,
            ((n >> 32) & 0xffff) as u16,
            ((n >> 48) & 0xfff) as u16,
            ((n >> 60) & 0xfff) as u16,
            (n.rotate_left(17) & 0xffff_ffff_ffff) as u64,
        )
    })
    .as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed but otherwise verbatim from the live endpoint, including the
    /// other sorts that surround the one we want.
    const REAL: &str = r#"{
      "pageType": "Home",
      "sorts": [
        {"topic":"Friends","topicId":600000000,"treatmentType":"FriendCarousel",
         "recommendationList":[]},
        {"topic":"Recommended For You","topicId":100000000,"treatmentType":"SortlessGrid",
         "recommendationList":[{"contentType":"Game","contentId":111,"contentStringId":""}]},
        {"topic":"Continue","topicId":100000003,"treatmentType":"Carousel",
         "recommendationList":[
           {"contentType":"Game","contentId":1359573625,"contentStringId":""},
           {"contentType":"Game","contentId":6371484684,"contentStringId":""},
           {"contentType":"Game","contentId":210851291,"contentStringId":""}]}
      ]
    }"#;

    #[test]
    fn the_continue_sort_is_picked_out_in_order() {
        let feed: Feed = serde_json::from_str(REAL).unwrap();
        assert_eq!(
            continue_universes(&feed, 12),
            vec![1359573625, 6371484684, 210851291]
        );
    }

    #[test]
    fn other_sorts_do_not_leak_in() {
        let feed: Feed = serde_json::from_str(REAL).unwrap();
        // 111 belongs to "Recommended For You" and must never appear here.
        assert!(!continue_universes(&feed, 12).contains(&111));
    }

    #[test]
    fn the_limit_is_respected() {
        let feed: Feed = serde_json::from_str(REAL).unwrap();
        assert_eq!(continue_universes(&feed, 2), vec![1359573625, 6371484684]);
    }

    #[test]
    fn a_feed_without_the_sort_is_empty_not_an_error() {
        let feed: Feed = serde_json::from_str(
            r#"{"sorts":[{"topic":"Friends","topicId":600000000,"recommendationList":[]}]}"#,
        )
        .unwrap();
        assert!(continue_universes(&feed, 12).is_empty());
    }

    #[test]
    fn a_localised_topic_name_still_works() {
        // The whole reason we match on the id: this is the Continue sort in
        // Swedish, and it must still be found.
        let feed: Feed = serde_json::from_str(
            r#"{"sorts":[{"topic":"Fortsätt","topicId":100000003,
                "recommendationList":[{"contentType":"Game","contentId":7}]}]}"#,
        )
        .unwrap();
        assert_eq!(continue_universes(&feed, 12), vec![7]);
    }

    #[test]
    fn non_game_rows_are_dropped() {
        let feed: Feed = serde_json::from_str(
            r#"{"sorts":[{"topicId":100000003,"recommendationList":[
                {"contentType":"RecommendedFriend","contentId":42},
                {"contentType":"Game","contentId":9}]}]}"#,
        )
        .unwrap();
        assert_eq!(continue_universes(&feed, 12), vec![9]);
    }

    #[test]
    fn the_session_id_is_guid_shaped_and_stable() {
        let a = session_id();
        assert_eq!(a, session_id());
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "not GUID-shaped: {a}"
        );
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}
