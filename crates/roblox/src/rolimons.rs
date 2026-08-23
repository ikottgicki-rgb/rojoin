//! Historical game statistics from Rolimons.
//!
//! Roblox tells a client what a game looks like *now* and keeps its past to
//! itself — every playtime or history endpoint answers 404. Rolimons has been
//! sampling public game stats every six hours since 2019, so this is the only
//! way to show a game's trend rather than a single instant.
//!
//! There is no documented API for it. Their `api.rolimons.com` game paths do not
//! exist, and any request without a browser `User-Agent` is refused outright.
//! What does work is the ordinary game page, which embeds the whole series in a
//! `<script id="game_history_json">` block — so that is what this reads.
//!
//! Two consequences worth being honest about:
//!   * it is scraping, and can break whenever they change their page,
//!   * the page is around 850 KB, so it must be fetched on demand and cached,
//!     never per tile in a grid.
//!
//! **This never sends the user's Roblox session anywhere.** It builds its own
//! HTTP client with no cookie jar rather than borrowing the authenticated one,
//! so a `.ROBLOSECURITY` cannot reach a third party even by accident.

use serde::Deserialize;

use crate::{Error, Result};

/// Enough of a browser to be served. Their edge rejects the app's normal
/// `RoJoin/x.y.z` agent with a 403 before any content is returned.
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// Sampling density is **not** uniform, so nothing here assumes an interval.
// A real series showed 6-hour spacing back in 2019 and roughly 10-minute
// spacing recently — 1009 points in the last week against 11323 over seven
// years. Anything that divided a span by a fixed interval would be wrong at one
// end or the other, which is why the chart buckets by time instead.

/// One game's history. Every series is parallel to `timestamps`.
///
/// `avg_playtime` is `None` for older samples — Rolimons started recording it
/// later — which is why it is an option rather than a zero.
/// Field names are **snake_case** here, unlike every Roblox endpoint in this
/// crate — Rolimons is a different service and does not follow their
/// conventions. A `rename_all = "camelCase"` silently mapped `avg_playtime` to
/// nothing and reported games as having no playtime data at all.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct History {
    /// Unix seconds, ascending.
    #[serde(deserialize_with = "crate::null_vec")]
    pub timestamps: Vec<i64>,
    #[serde(deserialize_with = "crate::null_vec")]
    pub players: Vec<Option<i64>>,
    #[serde(deserialize_with = "crate::null_vec")]
    pub avg_playtime: Vec<Option<f64>>,
    #[serde(deserialize_with = "crate::null_vec")]
    pub visits: Vec<Option<i64>>,
    #[serde(deserialize_with = "crate::null_vec")]
    pub upvotes: Vec<Option<i64>>,
    #[serde(deserialize_with = "crate::null_vec")]
    pub downvotes: Vec<Option<i64>>,
    #[serde(deserialize_with = "crate::null_vec")]
    pub favorites: Vec<Option<i64>>,
}

impl History {
    pub fn is_empty(&self) -> bool {
        self.timestamps.is_empty()
    }

    /// Samples no older than `days`, as `(timestamp, players)`.
    ///
    /// Truncated to the shortest of the two series: the arrays are supposed to
    /// be parallel, and indexing one by the other's length would panic if they
    /// ever were not.
    pub fn recent_players(&self, days: i64, now: i64) -> Vec<(i64, i64)> {
        let cutoff = now - days.max(1) * 86_400;
        let n = self.timestamps.len().min(self.players.len());

        (0..n)
            .filter(|i| self.timestamps[*i] >= cutoff)
            .filter_map(|i| self.players[i].map(|p| (self.timestamps[i], p)))
            .collect()
    }

    /// The most recent non-null value of a series.
    fn latest<T: Copy>(series: &[Option<T>]) -> Option<T> {
        series.iter().rev().find_map(|v| *v)
    }

    pub fn latest_players(&self) -> Option<i64> {
        Self::latest(&self.players)
    }

    pub fn latest_visits(&self) -> Option<i64> {
        Self::latest(&self.visits)
    }

    /// Minutes, as Rolimons reports it.
    pub fn latest_avg_playtime(&self) -> Option<f64> {
        Self::latest(&self.avg_playtime)
    }

    /// Rating as a percentage of votes that are positive.
    pub fn latest_rating(&self) -> Option<i32> {
        let up = Self::latest(&self.upvotes)?;
        let down = Self::latest(&self.downvotes)?;
        let total = up + down;
        if total <= 0 {
            return None;
        }
        Some(((up as f64 / total as f64) * 100.0).round() as i32)
    }

    /// Busiest sample in a window, and when it happened.
    pub fn peak(&self, days: i64, now: i64) -> Option<(i64, i64)> {
        self.recent_players(days, now).into_iter().max_by_key(|(_, p)| *p)
    }

    /// The span the data actually covers, in days.
    pub fn covered_days(&self) -> i64 {
        match (self.timestamps.first(), self.timestamps.last()) {
            (Some(a), Some(b)) => ((b - a) / 86_400).max(0),
            _ => 0,
        }
    }
}

/// Fetch and parse a game's history.
///
/// Keyed on **root place id**, which is what their URLs use — passing a universe
/// id returns somebody else's game or nothing at all.
pub async fn history(root_place_id: i64) -> Result<History> {
    if root_place_id <= 0 {
        return Err(Error::Api("no place id to look up".into()));
    }

    // Deliberately its own client: no cookie store, no shared state with the
    // authenticated Roblox one.
    let http = reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let url = format!("https://www.rolimons.com/game/{root_place_id}");
    let resp = http.get(&url).send().await?;

    if !resp.status().is_success() {
        return Err(Error::Api(format!(
            "Rolimons returned {} for place {root_place_id}",
            resp.status()
        )));
    }

    let html = resp.text().await?;
    extract(&html).ok_or_else(|| {
        Error::Api("Rolimons served a page with no history in it".into())
    })
}

/// Pull the embedded JSON out of the page.
///
/// Split out from the fetch so the parsing can be tested against a real payload
/// without going near the network.
pub fn extract(html: &str) -> Option<History> {
    const OPEN: &str = r#"<script id="game_history_json""#;

    let start = html.find(OPEN)?;
    // Past the tag's own attributes to the content itself.
    let body_start = start + html[start..].find('>')? + 1;
    let body_end = body_start + html[body_start..].find("</script>")?;

    let json = html[body_start..body_end].trim();
    let history: History = serde_json::from_str(json).ok()?;

    if history.is_empty() {
        return None;
    }
    Some(history)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real shape, trimmed. `avg_playtime` starts null exactly as it does
    /// live, because Rolimons added that series later than the others.
    const PAGE: &str = r#"<html><head></head><body>
        <script>var x = 1;</script>
        <script id="game_history_json" type="application/json">{
          "num_points":4,
          "timestamps":[1572847200,1572868800,1572890400,1572912000],
          "players":[8767,14026,24438,20000],
          "avg_playtime":[null,null,12.5,14.0],
          "visits":[3047335550,3047603819,3048113991,3048200000],
          "upvotes":[3143011,3143136,3143405,3143500],
          "downvotes":[423881,423902,423919,423920],
          "favorites":[12690922,12691390,12692302,12692400]
        }</script>
        </body></html>"#;

    fn parsed() -> History {
        extract(PAGE).expect("the real page shape must parse")
    }

    #[test]
    fn the_embedded_series_are_all_read() {
        let h = parsed();
        assert_eq!(h.timestamps.len(), 4);
        assert_eq!(h.players.len(), 4);
        assert_eq!(h.latest_players(), Some(20000));
        assert_eq!(h.latest_visits(), Some(3048200000));
        assert_eq!(h.latest_avg_playtime(), Some(14.0));
    }

    #[test]
    fn a_series_that_starts_null_still_yields_its_latest_value() {
        // avg_playtime is null for the first two samples; taking [0] or an
        // average over the lot would report nothing at all.
        let h = parsed();
        assert_eq!(h.latest_avg_playtime(), Some(14.0));
    }

    #[test]
    fn rating_is_the_share_of_positive_votes() {
        let h = parsed();
        // 3143500 / (3143500 + 423920) = 88.1%
        assert_eq!(h.latest_rating(), Some(88));
    }

    #[test]
    fn a_game_with_no_votes_has_no_rating_rather_than_zero_percent() {
        let h = History {
            upvotes: vec![Some(0)],
            downvotes: vec![Some(0)],
            ..Default::default()
        };
        assert_eq!(h.latest_rating(), None);
    }

    #[test]
    fn the_window_filters_by_age() {
        let h = parsed();
        let now = 1572912000;
        // Everything is inside a wide window.
        assert_eq!(h.recent_players(365 * 20, now).len(), 4);
        // And nothing is inside a one-day window a year after the last sample.
        assert!(h.recent_players(1, now + 400 * 86_400).is_empty());
    }

    #[test]
    fn the_peak_is_the_busiest_sample_not_the_last() {
        let h = parsed();
        let (_, peak) = h.peak(365 * 20, 1572912000).unwrap();
        assert_eq!(peak, 24438, "24438 is the high point, 20000 is merely latest");
    }

    #[test]
    fn mismatched_series_lengths_cannot_panic() {
        // Defensive: the arrays are meant to be parallel, and indexing one by
        // the other's length would be an out-of-bounds panic if they ever drift.
        let h = History {
            timestamps: vec![1, 2, 3, 4, 5],
            players: vec![Some(10)],
            ..Default::default()
        };
        assert_eq!(h.recent_players(365 * 100, 5), vec![(1, 10)]);
    }

    #[test]
    fn a_page_without_the_block_is_none_not_a_panic() {
        assert!(extract("<html><body>nothing here</body></html>").is_none());
        assert!(extract("").is_none());
        // Present but truncated mid-way.
        assert!(extract(r#"<script id="game_history_json">{"timesta"#).is_none());
    }

    #[test]
    fn an_empty_series_counts_as_no_history() {
        let page = r#"<script id="game_history_json">{"timestamps":[]}</script>"#;
        assert!(extract(page).is_none(), "empty history is not usable history");
    }

    #[test]
    fn nulls_where_arrays_are_expected_do_not_fail_the_parse() {
        let page = r#"<script id="game_history_json">
            {"timestamps":[1],"players":null,"visits":null}</script>"#;
        let h = extract(page).expect("a null series must not fail the whole page");
        assert!(h.players.is_empty());
    }

    #[test]
    fn covered_days_reports_the_real_span() {
        let h = History { timestamps: vec![0, 10 * 86_400], ..Default::default() };
        assert_eq!(h.covered_days(), 10);
        assert_eq!(History::default().covered_days(), 0);
    }
}

/// One column of the chart: a time bucket with the average concurrent players
/// seen in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    /// Start of the bucket, unix seconds.
    pub at: i64,
    pub players: i64,
}

/// Reduce a series to at most `columns` evenly-spaced buckets.
///
/// Necessary because the series is not evenly sampled: the last week of a
/// popular game carries about a thousand points while a year carries under
/// three thousand. Drawing one column per sample would compress recent history
/// into a smear and stretch old history into steps.
///
/// Buckets are spaced by *time*, not by sample count, so the x axis stays
/// linear — and each holds the mean of the samples inside it, which is what
/// "how busy was it around then" means. Empty buckets are dropped rather than
/// drawn as zero, since no sample is not the same as nobody playing.
pub fn bucket_players(points: &[(i64, i64)], columns: usize) -> Vec<Bucket> {
    if points.is_empty() || columns == 0 {
        return Vec::new();
    }

    let first = points.first().map(|(t, _)| *t).unwrap_or(0);
    let last = points.last().map(|(t, _)| *t).unwrap_or(0);
    let span = (last - first).max(1);

    // Fewer samples than columns: nothing to reduce.
    if points.len() <= columns {
        return points.iter().map(|(at, players)| Bucket { at: *at, players: *players }).collect();
    }

    let width = (span as f64 / columns as f64).max(1.0);
    let mut sums = vec![0i64; columns];
    let mut counts = vec![0u32; columns];

    for (at, players) in points {
        let idx = (((at - first) as f64) / width) as usize;
        let idx = idx.min(columns - 1);
        sums[idx] += players;
        counts[idx] += 1;
    }

    (0..columns)
        .filter(|i| counts[*i] > 0)
        .map(|i| Bucket {
            at: first + (i as f64 * width) as i64,
            players: sums[i] / counts[i] as i64,
        })
        .collect()
}

#[cfg(test)]
mod bucket_tests {
    use super::*;

    #[test]
    fn nothing_in_nothing_out() {
        assert!(bucket_players(&[], 10).is_empty());
        assert!(bucket_players(&[(1, 1)], 0).is_empty());
    }

    #[test]
    fn a_short_series_passes_through_untouched() {
        let pts = [(0, 5), (100, 7), (200, 9)];
        let out = bucket_players(&pts, 10);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].players, 5);
        assert_eq!(out[2].players, 9);
    }

    #[test]
    fn a_long_series_is_reduced_to_the_column_count() {
        let pts: Vec<(i64, i64)> = (0..1000).map(|i| (i * 600, 100 + i % 50)).collect();
        let out = bucket_players(&pts, 60);
        assert!(out.len() <= 60, "got {} columns", out.len());
        assert!(out.len() > 50, "should fill most columns: {}", out.len());
    }

    #[test]
    fn buckets_hold_the_mean_of_what_is_inside_them() {
        // Two samples in one bucket, averaging 20.
        let pts = [(0, 10), (1, 30)];
        let out = bucket_players(&pts, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].players, 20);
    }

    #[test]
    fn buckets_stay_in_time_order() {
        let pts: Vec<(i64, i64)> = (0..500).map(|i| (i * 3600, i)).collect();
        let out = bucket_players(&pts, 40);
        assert!(out.windows(2).all(|w| w[0].at <= w[1].at));
    }

    #[test]
    fn a_gap_in_sampling_leaves_a_gap_rather_than_a_zero() {
        // Dense early, dense late, nothing in between: the quiet middle must not
        // be drawn as "no players", which would read as the game dying.
        let mut pts: Vec<(i64, i64)> = (0..50).map(|i| (i * 60, 500)).collect();
        pts.extend((0..50).map(|i| (100_000 + i * 60, 500)));
        let out = bucket_players(&pts, 20);
        assert!(out.iter().all(|b| b.players == 500), "no invented zeroes");
        assert!(out.len() < 20, "the empty middle buckets are dropped");
    }

    #[test]
    fn an_unsorted_input_cannot_index_out_of_bounds() {
        // Defensive: first/last are taken positionally, so a series that is not
        // ascending could compute a negative offset.
        let pts = [(1000, 5), (0, 9)];
        let out = bucket_players(&pts, 4);
        assert!(out.len() <= 4);
    }
}
