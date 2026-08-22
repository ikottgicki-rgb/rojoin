//! Play-session records and the day-by-day aggregation the graph draws.
//!
//! Sessions are stored individually rather than as a running total per game.
//! A total answers "how much have I played this" and nothing else; individual
//! sessions answer that *and* when, how often, and for how long at a stretch,
//! which is what a graph needs. They are small — a few dozen bytes each — so
//! even an unlimited retention setting stays cheap for a single player.
//!
//! Nothing here talks to Roblox or to the UI, so all of it is directly
//! testable, which matters: an off-by-one in a date bucket is invisible on
//! screen until someone stares at the wrong day for a while.

use std::collections::HashMap;

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};

/// A game the platform would not name.
///
/// Roblox hides a user's game location according to *their* privacy settings,
/// so a friend can be plainly in a game while `universeId` comes back null.
/// The time is still real and still worth showing, so it lands here rather
/// than being thrown away.
pub const UNKNOWN_UNIVERSE: i64 = 0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlaySession {
    /// Universe id, or [`UNKNOWN_UNIVERSE`] when the platform would not say.
    pub universe_id: i64,
    /// Root place, so a row in the graph can still be launched.
    pub root_place_id: i64,
    pub name: String,
    /// Unix seconds at which the session was first observed.
    pub start: i64,
    /// Unix seconds at which it was last observed. Duration is the difference,
    /// which keeps a long session correct across app restarts: re-observing it
    /// extends the end rather than opening a second session.
    pub end: i64,
}

impl PlaySession {
    pub fn new(universe_id: i64, root_place_id: i64, name: &str, at: i64) -> Self {
        Self {
            universe_id,
            root_place_id,
            name: name.to_string(),
            start: at,
            end: at,
        }
    }

    pub fn secs(&self) -> u64 {
        self.end.saturating_sub(self.start).max(0) as u64
    }

    pub fn is_unknown_game(&self) -> bool {
        self.universe_id == UNKNOWN_UNIVERSE
    }
}

/// Append an observation to a session list.
///
/// Consecutive observations of the same game collapse into one session as long
/// as they are no further apart than `gap_secs`. That is what turns a sampled
/// presence poll into something that reads like a real session, and it means a
/// poll interval that drifts, or an app restart mid-game, does not shatter one
/// evening into a dozen fragments.
pub fn observe(
    sessions: &mut Vec<PlaySession>,
    universe_id: i64,
    root_place_id: i64,
    name: &str,
    at: i64,
    gap_secs: i64,
) {
    if let Some(last) = sessions.last_mut() {
        if last.universe_id == universe_id
            && at >= last.end
            && at - last.end <= gap_secs
        {
            last.end = at;
            // A later sighting may carry a name the earlier one lacked.
            if last.name.is_empty() && !name.is_empty() {
                last.name = name.to_string();
            }
            if last.root_place_id == 0 && root_place_id != 0 {
                last.root_place_id = root_place_id;
            }
            return;
        }
    }

    sessions.push(PlaySession::new(universe_id, root_place_id, name, at));
}

/// Drop sessions older than the retention window. `days == 0` keeps everything.
pub fn prune(sessions: &mut Vec<PlaySession>, days: u32, now: i64) {
    if days == 0 {
        return;
    }
    let cutoff = now - (days as i64) * 86_400;
    sessions.retain(|s| s.end >= cutoff);
}

/// One game's share of a single day.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSlice {
    pub universe_id: i64,
    pub root_place_id: i64,
    pub name: String,
    pub secs: u64,
}

/// One day of the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct DayBucket {
    /// Unix seconds at local midnight — the x position.
    pub midnight: i64,
    /// Short label for the axis, e.g. "Mon 18".
    pub label: String,
    pub total_secs: u64,
    /// Largest share first, so a stacked column reads big-to-small.
    pub games: Vec<GameSlice>,
}

/// Bucket sessions into the last `days` local days, oldest first.
///
/// Every day in the window is present even when nothing was played, because a
/// graph with missing columns misreads as "no data" rather than "played
/// nothing that day".
///
/// A session that straddles midnight is split across both days in proportion,
/// rather than being credited entirely to whichever end happens to win.
pub fn daily(sessions: &[PlaySession], days: u32, now: i64) -> Vec<DayBucket> {
    let days = days.max(1) as i64;
    let today = local_midnight(now);
    let first = today - (days - 1) * 86_400;

    // day midnight -> universe -> secs
    let mut grid: HashMap<i64, HashMap<i64, u64>> = HashMap::new();
    let mut names: HashMap<i64, (String, i64)> = HashMap::new();

    for s in sessions {
        if s.end < first || s.start > now {
            continue;
        }
        names
            .entry(s.universe_id)
            .and_modify(|slot| {
                if slot.0.is_empty() && !s.name.is_empty() {
                    slot.0 = s.name.clone();
                }
                if slot.1 == 0 {
                    slot.1 = s.root_place_id;
                }
            })
            .or_insert((s.name.clone(), s.root_place_id));

        // Walk the days the session touches and credit each its overlap.
        let mut day = local_midnight(s.start.max(first));
        while day <= local_midnight(s.end) {
            let day_end = day + 86_400;
            let from = s.start.max(day);
            let to = s.end.min(day_end);
            if to > from && day >= first {
                *grid.entry(day).or_default().entry(s.universe_id).or_insert(0) +=
                    (to - from) as u64;
            }
            day = day_end;
        }
    }

    (0..days)
        .map(|i| {
            let midnight = first + i * 86_400;
            let mut games: Vec<GameSlice> = grid
                .get(&midnight)
                .map(|per_game| {
                    per_game
                        .iter()
                        .map(|(universe, secs)| {
                            let (name, root) = names.get(universe).cloned().unwrap_or_default();
                            GameSlice {
                                universe_id: *universe,
                                root_place_id: root,
                                name,
                                secs: *secs,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            // Largest first, then by id so the order never wobbles between
            // renders for two games with identical time.
            games.sort_by(|a, b| b.secs.cmp(&a.secs).then(a.universe_id.cmp(&b.universe_id)));

            DayBucket {
                midnight,
                label: day_label(midnight),
                total_secs: games.iter().map(|g| g.secs).sum(),
                games,
            }
        })
        .collect()
}

/// Total time per game across the whole list, largest first.
pub fn totals(sessions: &[PlaySession]) -> Vec<GameSlice> {
    let mut per_game: HashMap<i64, GameSlice> = HashMap::new();

    for s in sessions {
        let slot = per_game.entry(s.universe_id).or_insert_with(|| GameSlice {
            universe_id: s.universe_id,
            root_place_id: s.root_place_id,
            name: s.name.clone(),
            secs: 0,
        });
        slot.secs += s.secs();
        if slot.name.is_empty() && !s.name.is_empty() {
            slot.name = s.name.clone();
        }
        if slot.root_place_id == 0 {
            slot.root_place_id = s.root_place_id;
        }
    }

    let mut out: Vec<GameSlice> = per_game.into_values().collect();
    out.sort_by(|a, b| b.secs.cmp(&a.secs).then(a.universe_id.cmp(&b.universe_id)));
    out
}

/// The longest single session in the list.
pub fn longest(sessions: &[PlaySession]) -> Option<&PlaySession> {
    sessions.iter().max_by_key(|s| s.secs())
}

/// Local midnight for the day containing `unix`.
///
/// Local, not UTC: a graph of *your* days has to break where your days break,
/// or an evening session lands on tomorrow for anyone west of UTC.
fn local_midnight(unix: i64) -> i64 {
    let dt = Local.timestamp_opt(unix, 0).single();
    match dt {
        Some(dt) => dt
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|naive| Local.from_local_datetime(&naive).single())
            .map(|d| d.timestamp())
            .unwrap_or(unix - unix.rem_euclid(86_400)),
        None => unix - unix.rem_euclid(86_400),
    }
}

fn day_label(midnight: i64) -> String {
    Local
        .timestamp_opt(midnight, 0)
        .single()
        .map(|d| d.format("%a %-d").to_string())
        .unwrap_or_default()
}

/// Days between two instants, for "tracking since" style copy.
pub fn days_between(from: i64, to: i64) -> i64 {
    (local_midnight(to) - local_midnight(from)) / 86_400 + 1
}

/// A pleasant y-axis ceiling: the next round number above the busiest day.
///
/// Scaling bars to the exact maximum makes the tallest column touch the top of
/// the plot on every single render, which reads as clipped.
pub fn axis_ceiling_secs(buckets: &[DayBucket]) -> u64 {
    let peak = buckets.iter().map(|b| b.total_secs).max().unwrap_or(0);
    if peak == 0 {
        return 3600;
    }
    for step in [
        900, 1800, 3600, 7200, 10800, 14400, 21600, 28800, 43200, 57600, 86400,
    ] {
        if peak <= step {
            return step;
        }
    }
    // Past a full day, round up to whole days.
    ((peak + 86_399) / 86_400) * 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed local midnight to build cases from, so tests do not depend on
    /// when they run.
    fn base() -> i64 {
        local_midnight(1_700_000_000)
    }

    fn session(universe: i64, start: i64, secs: i64) -> PlaySession {
        PlaySession {
            universe_id: universe,
            root_place_id: universe * 10,
            name: format!("Game {universe}"),
            start,
            end: start + secs,
        }
    }

    #[test]
    fn a_sampled_run_collapses_into_one_session() {
        let mut v = Vec::new();
        let t = base() + 3600;
        for i in 0..5 {
            observe(&mut v, 7, 70, "Game 7", t + i * 60, 180);
        }
        assert_eq!(v.len(), 1, "one continuous run is one session");
        assert_eq!(v[0].secs(), 240);
    }

    #[test]
    fn a_long_gap_starts_a_new_session() {
        let mut v = Vec::new();
        let t = base() + 3600;
        observe(&mut v, 7, 70, "Game 7", t, 180);
        observe(&mut v, 7, 70, "Game 7", t + 60, 180);
        // Ten minutes later is a different sitting.
        observe(&mut v, 7, 70, "Game 7", t + 660, 180);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn switching_game_starts_a_new_session() {
        let mut v = Vec::new();
        let t = base() + 3600;
        observe(&mut v, 7, 70, "Game 7", t, 180);
        observe(&mut v, 8, 80, "Game 8", t + 60, 180);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].universe_id, 7);
        assert_eq!(v[1].universe_id, 8);
    }

    #[test]
    fn a_later_sighting_fills_in_a_name_the_first_one_lacked() {
        let mut v = Vec::new();
        let t = base() + 3600;
        observe(&mut v, 7, 0, "", t, 180);
        observe(&mut v, 7, 70, "Game 7", t + 60, 180);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Game 7");
        assert_eq!(v[0].root_place_id, 70);
    }

    #[test]
    fn retention_drops_old_sessions_and_zero_keeps_everything() {
        let now = base() + 86_400;
        let mut v = vec![
            session(1, now - 100 * 86_400, 600),
            session(2, now - 2 * 86_400, 600),
        ];

        let mut kept = v.clone();
        prune(&mut kept, 0, now);
        assert_eq!(kept.len(), 2, "0 means unlimited");

        prune(&mut v, 90, now);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].universe_id, 2);
    }

    #[test]
    fn every_day_in_the_window_gets_a_column() {
        let now = base() + 43_200;
        let buckets = daily(&[session(1, base() + 3600, 600)], 7, now);
        assert_eq!(buckets.len(), 7);
        assert!(buckets.iter().all(|b| !b.label.is_empty()));
        // Oldest first.
        assert!(buckets[0].midnight < buckets[6].midnight);
        // Only today has time on it.
        assert_eq!(buckets[6].total_secs, 600);
        assert_eq!(buckets[0].total_secs, 0);
    }

    #[test]
    fn a_day_is_split_by_game_largest_first() {
        let now = base() + 43_200;
        let sessions = vec![
            session(1, base() + 3600, 600),
            session(2, base() + 7200, 1800),
            session(1, base() + 10800, 300),
        ];
        let buckets = daily(&sessions, 1, now);
        assert_eq!(buckets.len(), 1);
        let day = &buckets[0];
        assert_eq!(day.total_secs, 2700);
        assert_eq!(day.games[0].universe_id, 2, "biggest share first");
        assert_eq!(day.games[0].secs, 1800);
        assert_eq!(day.games[1].secs, 900, "both sessions of game 1 add up");
    }

    #[test]
    fn a_session_across_midnight_is_split_between_the_days() {
        // Starts 23:00, runs two hours: one hour each side of midnight.
        let start = base() + 23 * 3600;
        let now = base() + 86_400 + 43_200;
        let buckets = daily(&[session(1, start, 7200)], 2, now);
        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].total_secs, 3600, "yesterday keeps its hour");
        assert_eq!(buckets[1].total_secs, 3600, "today keeps its hour");
    }

    #[test]
    fn sessions_outside_the_window_are_ignored() {
        let now = base() + 43_200;
        let buckets = daily(&[session(1, base() - 30 * 86_400, 3600)], 7, now);
        assert!(buckets.iter().all(|b| b.total_secs == 0));
    }

    #[test]
    fn totals_add_every_session_per_game() {
        let t = base();
        let out = totals(&[
            session(1, t, 600),
            session(2, t, 100),
            session(1, t + 5000, 400),
        ]);
        assert_eq!(out[0].universe_id, 1);
        assert_eq!(out[0].secs, 1000);
        assert_eq!(out[1].secs, 100);
    }

    #[test]
    fn an_unnamed_game_is_still_counted() {
        let mut s = session(UNKNOWN_UNIVERSE, base() + 3600, 900);
        s.name = String::new();
        assert!(s.is_unknown_game());
        let buckets = daily(&[s], 1, base() + 43_200);
        assert_eq!(buckets[0].total_secs, 900, "hidden game, real time");
    }

    #[test]
    fn the_axis_leaves_headroom_above_the_busiest_day() {
        let empty: Vec<DayBucket> = Vec::new();
        assert_eq!(axis_ceiling_secs(&empty), 3600);

        let buckets = daily(&[session(1, base() + 3600, 5000)], 1, base() + 43_200);
        let ceil = axis_ceiling_secs(&buckets);
        assert!(ceil >= 5000, "ceiling must not clip the tallest bar");
        assert_eq!(ceil, 7200);
    }

    #[test]
    fn the_longest_session_is_found() {
        let t = base();
        let list = [session(1, t, 600), session(2, t + 5000, 3600), session(3, t + 20000, 60)];
        assert_eq!(longest(&list).unwrap().universe_id, 2);
    }

    #[test]
    fn a_session_never_reports_negative_time() {
        let s = PlaySession { start: 500, end: 100, ..Default::default() };
        assert_eq!(s.secs(), 0);
    }
}
