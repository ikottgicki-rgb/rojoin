//! Our own history of a game's public statistics.
//!
//! Roblox reports what a game looks like *now* and keeps its past to itself, so
//! the only way to draw a trend is to remember what we saw. Every figure here
//! comes from the same public endpoints the app already calls to render a game
//! page — `playing`, `visits` and the vote counts — so recording them costs no
//! extra requests and involves nobody else's data.
//!
//! This replaces reading the same numbers off Rolimons. Their terms prohibit
//! automated access and redisplay of their data, and a manual trigger would not
//! have changed that, so the app collects its own. The trade is honest: it starts
//! empty and fills in as you browse, rather than arriving with years of history.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// One observation of a game.
///
/// Vote totals rather than a percentage: a rating can be derived from them, but
/// not the other way round, and storing the raw pair means a later change of
/// mind about presentation costs nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sample {
    /// Unix seconds.
    pub at: i64,
    pub playing: i64,
    pub visits: i64,
    pub upvotes: i64,
    pub downvotes: i64,
}

impl Sample {
    /// Share of votes that are positive.
    pub fn rating(&self) -> Option<i32> {
        let total = self.upvotes + self.downvotes;
        if total <= 0 {
            return None;
        }
        Some(((self.upvotes as f64 / total as f64) * 100.0).round() as i32)
    }
}

/// Samples for one game, oldest first.
pub type Series = Vec<Sample>;

/// Don't store a sample if the last one is newer than this.
///
/// Opening a game page three times in a minute is one data point, not three, and
/// without this the series would be dense where someone happened to click a lot
/// and sparse everywhere else — which is exactly the uneven sampling that makes
/// a chart lie.
pub const MIN_GAP_SECS: i64 = 900;

/// Add an observation, unless one was taken recently.
///
/// Returns whether anything was stored, so the caller can skip saving the file.
pub fn record(series: &mut Series, sample: Sample, keep_days: u32) -> bool {
    if let Some(last) = series.last() {
        if sample.at - last.at < MIN_GAP_SECS {
            return false;
        }
        // A clock that went backwards must not create a series that is not
        // ascending, since everything downstream assumes it is.
        if sample.at < last.at {
            return false;
        }
    }

    series.push(sample);
    prune(series, keep_days, sample.at);
    true
}

/// Drop samples older than the window. `days == 0` keeps everything.
pub fn prune(series: &mut Series, days: u32, now: i64) {
    if days == 0 {
        return;
    }
    let cutoff = now - (days as i64) * 86_400;
    series.retain(|s| s.at >= cutoff);
}

/// Samples no older than `days`.
pub fn recent(series: &[Sample], days: i64, now: i64) -> Vec<Sample> {
    let cutoff = now - days.max(1) * 86_400;
    series.iter().copied().filter(|s| s.at >= cutoff).collect()
}

/// One column of a chart: a time bucket and the mean player count in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    pub at: i64,
    pub playing: i64,
}

/// Reduce a series to at most `columns` buckets, spaced by time.
///
/// Spaced by *time* rather than by sample count so the x axis stays linear even
/// though browsing habits make the sampling uneven. Empty buckets are dropped
/// instead of drawn as zero, because no observation is not the same as nobody
/// playing.
pub fn bucket(samples: &[Sample], columns: usize) -> Vec<Bucket> {
    if samples.is_empty() || columns == 0 {
        return Vec::new();
    }
    if samples.len() <= columns {
        return samples.iter().map(|s| Bucket { at: s.at, playing: s.playing }).collect();
    }

    let first = samples.first().map(|s| s.at).unwrap_or(0);
    let last = samples.last().map(|s| s.at).unwrap_or(0);
    let span = (last - first).max(1);
    let width = (span as f64 / columns as f64).max(1.0);

    let mut sums = vec![0i64; columns];
    let mut counts = vec![0u32; columns];

    for s in samples {
        let idx = (((s.at - first).max(0) as f64) / width) as usize;
        let idx = idx.min(columns - 1);
        sums[idx] += s.playing;
        counts[idx] += 1;
    }

    (0..columns)
        .filter(|i| counts[*i] > 0)
        .map(|i| Bucket {
            at: first + (i as f64 * width) as i64,
            playing: sums[i] / counts[i] as i64,
        })
        .collect()
}

/// Every game we have samples for, keyed by root place id as a string.
pub type Store = HashMap<String, Series>;

/// The span a series covers, in days.
pub fn covered_days(series: &[Sample]) -> i64 {
    match (series.first(), series.last()) {
        (Some(a), Some(b)) => ((b.at - a.at) / 86_400).max(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: i64, playing: i64) -> Sample {
        Sample { at, playing, visits: 1_000, upvotes: 90, downvotes: 10 }
    }

    #[test]
    fn a_rating_comes_from_the_vote_pair() {
        assert_eq!(sample(0, 0).rating(), Some(90));
        assert_eq!(Sample::default().rating(), None, "no votes is not 0%");
    }

    #[test]
    fn the_first_sample_is_always_stored() {
        let mut s = Series::new();
        assert!(record(&mut s, sample(1_000, 10), 90));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn opening_a_page_repeatedly_is_one_data_point() {
        let mut s = Series::new();
        assert!(record(&mut s, sample(10_000, 10), 90));
        assert!(!record(&mut s, sample(10_060, 11), 90), "a minute later");
        assert!(!record(&mut s, sample(10_500, 12), 90), "eight minutes later");
        assert_eq!(s.len(), 1);

        assert!(record(&mut s, sample(10_000 + MIN_GAP_SECS, 13), 90));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn a_backwards_clock_cannot_break_the_ordering() {
        let mut s = Series::new();
        record(&mut s, sample(10_000, 10), 90);
        assert!(!record(&mut s, sample(5_000, 11), 90));
        assert!(s.windows(2).all(|w| w[0].at <= w[1].at));
    }

    #[test]
    fn retention_drops_old_samples_and_zero_keeps_everything() {
        let now = 100 * 86_400;
        let mut s = vec![sample(1_000, 5), sample(now - 86_400, 6)];
        prune(&mut s, 0, now);
        assert_eq!(s.len(), 2);
        prune(&mut s, 30, now);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn recent_filters_by_age() {
        let now = 100 * 86_400;
        let s = vec![sample(now - 60 * 86_400, 1), sample(now - 86_400, 2)];
        assert_eq!(recent(&s, 30, now).len(), 1);
        assert_eq!(recent(&s, 365, now).len(), 2);
    }

    #[test]
    fn a_short_series_buckets_one_to_one() {
        let s: Vec<Sample> = (0..5).map(|i| sample(i * 3600, i * 10)).collect();
        assert_eq!(bucket(&s, 50).len(), 5);
    }

    #[test]
    fn a_long_series_is_reduced_to_the_column_count() {
        let s: Vec<Sample> = (0..2000).map(|i| sample(i * 900, 100 + i % 40)).collect();
        let out = bucket(&s, 60);
        assert!(out.len() <= 60, "got {}", out.len());
        assert!(out.len() > 50);
    }

    #[test]
    fn a_bucket_holds_the_mean_of_its_contents() {
        let s = vec![sample(0, 10), sample(1, 30)];
        let out = bucket(&s, 1);
        assert_eq!(out[0].playing, 20);
    }

    #[test]
    fn a_gap_is_a_gap_rather_than_a_run_of_zeroes() {
        let mut s: Vec<Sample> = (0..40).map(|i| sample(i * 60, 500)).collect();
        s.extend((0..40).map(|i| sample(200_000 + i * 60, 500)));
        let out = bucket(&s, 20);
        assert!(out.iter().all(|b| b.playing == 500), "no invented zeroes");
        assert!(out.len() < 20);
    }

    #[test]
    fn buckets_stay_in_order() {
        let s: Vec<Sample> = (0..600).map(|i| sample(i * 1800, i)).collect();
        let out = bucket(&s, 40);
        assert!(out.windows(2).all(|w| w[0].at <= w[1].at));
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(bucket(&[], 10).is_empty());
        assert!(bucket(&[sample(0, 1)], 0).is_empty());
        assert_eq!(covered_days(&[]), 0);
    }

    #[test]
    fn covered_days_reports_the_real_span() {
        let s = vec![sample(0, 1), sample(10 * 86_400, 2)];
        assert_eq!(covered_days(&s), 10);
    }
}
