//! Core models -> Slint structs, plus the formatters the UI needs.
//!
//! Roblox ids become `string` here: Slint's `int` is i32 and place/user ids
//! already exceed that range. Everything that renders a number the user reads
//! (player counts, visits, dates, durations) is formatted here rather than in
//! `.slint`, because Slint has no formatting primitives worth the name.

use rojoin_roblox::models::{Badge, GameDetail, GamePass, Place, Server, Votes};
use rojoin_roblox::search::OmniGame;
use slint::{Image, ModelRc, SharedString, VecModel};
use std::rc::Rc;

use crate::{DetailItem, GameDetailData, GameTile, ServerRow, SubPlace};

/// Compact player/visit counts: 1234 -> "1.2K", 4500000 -> "4.5M".
pub fn compact(n: i64) -> String {
    match n {
        n if n < 0 => "0".into(),
        n if n < 1_000 => n.to_string(),
        n if n < 1_000_000 => {
            let v = n as f64 / 1_000.0;
            if v < 10.0 { format!("{v:.1}K") } else { format!("{:.0}K", v) }
        }
        n if n < 1_000_000_000 => {
            let v = n as f64 / 1_000_000.0;
            if v < 10.0 { format!("{v:.1}M") } else { format!("{:.0}M", v) }
        }
        n => format!("{:.1}B", n as f64 / 1_000_000_000.0),
    }
}

/// RFC3339 -> "2 Jan 2020". Falls back to an empty string rather than showing
/// the user a raw timestamp.
pub fn fmt_date(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|d| d.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

pub fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=21 => "Good evening",
        _ => "Good evening",
    }
}

/// Seconds -> "4h 20m" / "45m" / "30s". Never renders "0h 0m".
pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0 => "0m".into(),
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m", s / 60),
        s => {
            let hours = s / 3_600;
            let mins = (s % 3_600) / 60;
            if mins == 0 {
                format!("{hours}h")
            } else {
                format!("{hours}h {mins}m")
            }
        }
    }
}

pub fn today_label() -> String {
    chrono::Local::now().format("%A, %-d %B").to_string()
}

pub fn tile_from_detail(d: &GameDetail, votes: Option<&Votes>, favorited: bool) -> GameTile {
    GameTile {
        id: d.root_place_id.to_string().into(),
        universe_id: d.id.to_string().into(),
        name: d.name.clone().into(),
        creator: d.creator.name.clone().into(),
        playing: compact(d.playing).into(),
        rating: votes.and_then(Votes::percent).unwrap_or(-1),
        thumb: Image::default(),
        favorited,
    }
}

pub fn tile_from_omni(g: &OmniGame) -> GameTile {
    GameTile {
        id: g.root_place_id.to_string().into(),
        universe_id: g.universe_id.to_string().into(),
        name: g.name.clone().into(),
        creator: g.creator_name.clone().into(),
        playing: compact(g.player_count).into(),
        rating: g.percent().unwrap_or(-1),
        thumb: Image::default(),
        favorited: false,
    }
}

pub fn detail_data(d: &GameDetail, votes: Option<&Votes>, notify: bool) -> GameDetailData {
    GameDetailData {
        universe_id: d.id.to_string().into(),
        root_place_id: d.root_place_id.to_string().into(),
        name: d.name.clone().into(),
        creator: d.creator.name.clone().into(),
        creator_id: d.creator.id.to_string().into(),
        creator_is_group: d.creator.kind.eq_ignore_ascii_case("Group"),
        description: d.description.clone().unwrap_or_default().into(),
        playing: compact(d.playing).into(),
        visits: compact(d.visits).into(),
        favorites: compact(d.favorited_count).into(),
        max_players: d.max_players.to_string().into(),
        created: fmt_date(&d.created).into(),
        updated: fmt_date(&d.updated).into(),
        genre: if d.genre.as_deref().unwrap_or("").is_empty() {
            "—".into()
        } else {
            d.genre.clone().unwrap_or_default().into()
        },
        rating: votes.and_then(Votes::percent).unwrap_or(-1),
        favorited: d.is_favorited_by_user,
        notify,
        icon: Image::default(),
        hero: Image::default(),
    }
}

pub fn sub_places(places: &[Place]) -> Vec<SubPlace> {
    places
        .iter()
        .map(|p| SubPlace {
            id: p.id.to_string().into(),
            name: p.name.clone().into(),
            description: p
                .description
                .clone()
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(90)
                .collect::<String>()
                .into(),
        })
        .collect()
}

pub fn servers(list: &[Server]) -> Vec<ServerRow> {
    list.iter()
        .map(|s| {
            let shown = s.player_tokens.as_ref().map(Vec::len).unwrap_or(0).min(4);
            ServerRow {
                id: s.id.clone().into(),
                playing: s.playing,
                max: s.max_players,
                label: format!("{} / {}", s.playing, s.max_players).into(),
                fill: if s.max_players > 0 {
                    (s.playing as f32 / s.max_players as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                },
                p0: Image::default(),
                p1: Image::default(),
                p2: Image::default(),
                p3: Image::default(),
                more: (s.playing - shown as i32).max(0),
            }
        })
        .collect()
}

pub fn passes(list: &[GamePass]) -> Vec<DetailItem> {
    list.iter()
        .map(|p| DetailItem {
            id: p.id.to_string().into(),
            name: p.name.clone().into(),
            subtitle: SharedString::default(),
            thumb: Image::default(),
            kind: 2,
        })
        .collect()
}

pub fn badges(list: &[Badge]) -> Vec<DetailItem> {
    list.iter()
        .map(|b| DetailItem {
            id: b.id.to_string().into(),
            name: b.name.clone().into(),
            subtitle: b.description.clone().unwrap_or_default().into(),
            thumb: Image::default(),
            kind: 3,
        })
        .collect()
}

/// "3m ago", "2h ago", "5d ago". Empty for anything unparseable, because a raw
/// timestamp under someone's name is worse than no subtitle at all.
/// "3h ago" from two unix timestamps.
///
/// The ISO-string variant below exists because Roblox sends dates that way;
/// play sessions are already numbers, so they take the short path.
pub fn time_ago_unix(then: i64, now: i64) -> String {
    let secs = (now - then).max(0);
    match secs {
        s if s < 90 => "just now".into(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s if s < 7 * 86_400 => format!("{}d ago", s / 86_400),
        s => format!("{}w ago", s / (7 * 86_400)),
    }
}

pub fn time_ago(iso: &str) -> String {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    let secs = (chrono::Utc::now() - then.with_timezone(&chrono::Utc)).num_seconds();

    match secs {
        s if s < 0 => "just now".into(),
        s if s < 60 => "just now".into(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s if s < 2_592_000 => format!("{}d ago", s / 86_400),
        s => format!("{}mo ago", s / 2_592_000),
    }
}

pub struct FriendInput {
    pub id: i64,
    pub name: String,
    pub username: String,
    pub presence: i32,
    pub location: String,
    pub last_online: String,
    pub place_id: Option<i64>,
    /// The specific server instance, so "Join" lands in *their* server rather
    /// than a random one.
    pub game_id: Option<String>,
    pub avatar_url: String,
    pub joinable: bool,
}

pub struct FriendsView {
    pub rows: Vec<crate::FriendRow>,
    pub in_game: i32,
    pub online: i32,
}

/// Flatten friends into the grouped, header-interleaved list the UI renders.
///
/// Pinned friends are lifted to their own section at the top *and removed from
/// the groups below*, so nobody is listed twice.
pub fn friend_rows(
    friends: &[FriendInput],
    pinned: &std::collections::HashSet<String>,
    notify: &std::collections::HashSet<String>,
    filter: &str,
    offline_collapsed: bool,
) -> FriendsView {
    let needle = filter.trim().to_lowercase();
    let matches = |f: &FriendInput| {
        needle.is_empty()
            || f.name.to_lowercase().contains(&needle)
            || f.username.to_lowercase().contains(&needle)
    };

    let in_game = friends.iter().filter(|f| f.presence == 2).count() as i32;
    let online = friends.iter().filter(|f| f.presence == 1 || f.presence == 3).count() as i32;

    let visible: Vec<&FriendInput> = friends.iter().filter(|f| matches(f)).collect();

    let make = |f: &FriendInput| crate::FriendRow {
        id: f.id.to_string().into(),
        name: f.name.clone().into(),
        username: f.username.clone().into(),
        subtitle: subtitle_for(f).into(),
        avatar: Image::default(),
        presence: f.presence,
        joinable: f.joinable,
        pinned: pinned.contains(&f.id.to_string()),
        notify: notify.contains(&f.id.to_string()),
        place_id: f.place_id.map(|p| p.to_string()).unwrap_or_default().into(),
        is_header: false,
        header_label: SharedString::default(),
        collapsible: false,
        collapsed: false,
    };

    let header = |label: &str, collapsible: bool, collapsed: bool| crate::FriendRow {
        is_header: true,
        header_label: label.into(),
        collapsible,
        collapsed,
        id: SharedString::default(),
        name: SharedString::default(),
        username: SharedString::default(),
        subtitle: SharedString::default(),
        avatar: Image::default(),
        presence: 0,
        joinable: false,
        pinned: false,
        notify: false,
        place_id: SharedString::default(),
    };

    let mut rows = Vec::new();

    let mut pins: Vec<&&FriendInput> = visible
        .iter()
        .filter(|f| pinned.contains(&f.id.to_string()))
        .collect();
    pins.sort_by_key(|f| (presence_rank(f.presence), f.name.to_lowercase()));

    if !pins.is_empty() {
        rows.push(header("PINNED", false, false));
        rows.extend(pins.iter().map(|f| make(f)));
    }

    let rest: Vec<&&FriendInput> = visible
        .iter()
        .filter(|f| !pinned.contains(&f.id.to_string()))
        .collect();

    let push_group = |rows: &mut Vec<crate::FriendRow>,
                          label: &str,
                          keep: &dyn Fn(i32) -> bool,
                          collapsible: bool,
                          collapsed: bool| {
        let mut group: Vec<&&&FriendInput> = rest.iter().filter(|f| keep(f.presence)).collect();
        if group.is_empty() {
            return;
        }
        group.sort_by_key(|f| f.name.to_lowercase());
        rows.push(header(label, collapsible, collapsed));
        if !collapsed {
            rows.extend(group.iter().map(|f| make(f)));
        }
    };

    push_group(&mut rows, "IN GAME", &|p| p == 2, false, false);
    push_group(&mut rows, "ONLINE", &|p| p == 1 || p == 3, false, false);
    push_group(&mut rows, "OFFLINE", &|p| p == 0 || p == 4, true, offline_collapsed);

    FriendsView { rows, in_game, online }
}

fn presence_rank(presence: i32) -> u8 {
    match presence {
        2 => 0,
        1 => 1,
        3 => 2,
        _ => 3,
    }
}

/// A short "what are they up to" line straight from a presence record.
///
/// Same wording as the friends list, but for someone who is not a friend yet
/// and so has no `FriendInput` to go through.
pub fn presence_label(p: Option<&rojoin_roblox::friends::Presence>) -> String {
    use rojoin_roblox::friends::PresenceKind;

    let Some(p) = p else { return String::new() };
    match p.kind {
        PresenceKind::InGame => {
            if p.location.is_empty() {
                "In game".into()
            } else {
                format!("Playing {}", p.location)
            }
        }
        PresenceKind::Online => "Online".into(),
        PresenceKind::InStudio => "In Studio".into(),
        _ => String::new(),
    }
}

fn subtitle_for(f: &FriendInput) -> String {
    match f.presence {
        2 => {
            if f.location.is_empty() {
                "In game".into()
            } else {
                format!("Playing {}", f.location)
            }
        }
        1 => "Online".into(),
        3 => "In Studio".into(),
        _ => {
            let ago = time_ago(&f.last_online);
            if ago.is_empty() {
                "Offline".into()
            } else {
                format!("Last seen {ago}")
            }
        }
    }
}

pub fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

pub fn strings(items: Vec<String>) -> ModelRc<SharedString> {
    model(items.into_iter().map(SharedString::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_counts_read_the_way_people_expect() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1.0K");
        assert_eq!(compact(12_400), "12K");
        assert_eq!(compact(1_500_000), "1.5M");
        assert_eq!(compact(45_000_000), "45M");
        assert_eq!(compact(8_200_000_000), "8.2B");
    }

    #[test]
    fn compact_never_renders_a_negative() {
        assert_eq!(compact(-5), "0");
    }

    #[test]
    fn dates_fall_back_to_empty_not_a_raw_timestamp() {
        assert_eq!(fmt_date("2020-01-02T03:04:05Z"), "2 Jan 2020");
        assert_eq!(fmt_date("garbage"), "");
        assert_eq!(fmt_date(""), "");
    }

    #[test]
    fn durations_never_render_a_zero_component() {
        assert_eq!(fmt_duration(0), "0m");
        assert_eq!(fmt_duration(30), "30s");
        assert_eq!(fmt_duration(90), "1m");
        assert_eq!(fmt_duration(3_600), "1h");
        assert_eq!(fmt_duration(3_600 + 1_200), "1h 20m");
        assert_eq!(fmt_duration(86_400), "24h");
    }

    #[test]
    fn greeting_covers_every_hour() {
        for h in 0..24 {
            assert!(!greeting(h).is_empty(), "hour {h} produced no greeting");
        }
        assert_eq!(greeting(9), "Good morning");
        assert_eq!(greeting(14), "Good afternoon");
        assert_eq!(greeting(20), "Good evening");
        assert_eq!(greeting(3), "Good evening");
    }

    #[test]
    fn server_fill_is_clamped_and_safe_at_zero_capacity() {
        let s = Server { id: "a".into(), max_players: 0, playing: 5, ..Default::default() };
        assert_eq!(servers(&[s])[0].fill, 0.0, "must not divide by zero");

        let s = Server { id: "b".into(), max_players: 10, playing: 20, ..Default::default() };
        assert_eq!(servers(&[s])[0].fill, 1.0, "over-full must clamp, not overflow the bar");
    }

    #[test]
    fn server_more_count_never_goes_negative() {
        let s = Server {
            id: "c".into(),
            max_players: 30,
            playing: 2,
            player_tokens: Some(vec!["a".into(), "b".into(), "c".into(), "d".into()]),
            ..Default::default()
        };
        assert_eq!(servers(&[s])[0].more, 0);
    }

    #[test]
    fn sub_place_description_is_trimmed_to_one_short_line() {
        let p = Place {
            id: 1,
            universe_id: 2,
            name: "Arena".into(),
            description: Some(format!("first line\nsecond line{}", "x".repeat(200))),
        };
        let out = sub_places(&[p]);
        assert_eq!(out[0].description, "first line");
    }

    #[test]
    fn ids_survive_as_strings_beyond_i32() {
        let d = GameDetail { root_place_id: 6_068_496_210, ..Default::default() };
        let t = tile_from_detail(&d, None, false);
        assert_eq!(t.id, "6068496210");
    }

    #[test]
    fn unvoted_game_gets_negative_rating_sentinel() {
        let d = GameDetail::default();
        assert_eq!(tile_from_detail(&d, None, false).rating, -1);

        let v = Votes { id: 1, up_votes: 0, down_votes: 0 };
        assert_eq!(tile_from_detail(&d, Some(&v), false).rating, -1);
    }

    fn friend(id: i64, name: &str, presence: i32) -> FriendInput {
        FriendInput {
            id,
            name: name.into(),
            username: name.to_lowercase(),
            presence,
            location: if presence == 2 { "Jailbreak".into() } else { String::new() },
            last_online: String::new(),
            place_id: (presence == 2).then_some(606849621),
            game_id: None,
            avatar_url: String::new(),
            joinable: presence == 2,
        }
    }

    fn empty() -> std::collections::HashSet<String> {
        std::collections::HashSet::new()
    }

    #[test]
    fn groups_are_ordered_in_game_then_online_then_offline() {
        let friends = vec![
            friend(1, "Offliner", 0),
            friend(2, "Gamer", 2),
            friend(3, "Onliner", 1),
        ];
        let view = friend_rows(&friends, &empty(), &empty(), "", false);

        let headers: Vec<String> = view
            .rows
            .iter()
            .filter(|r| r.is_header)
            .map(|r| r.header_label.to_string())
            .collect();
        assert_eq!(headers, vec!["IN GAME", "ONLINE", "OFFLINE"]);
        assert_eq!(view.in_game, 1);
        assert_eq!(view.online, 1);
    }

    #[test]
    fn pinned_friends_appear_once_only() {
        let friends = vec![friend(1, "Alice", 2), friend(2, "Bob", 1)];
        let mut pins = empty();
        pins.insert("1".to_string());

        let view = friend_rows(&friends, &pins, &empty(), "", false);
        let alice_rows = view.rows.iter().filter(|r| !r.is_header && r.id == "1").count();
        assert_eq!(alice_rows, 1, "pinned friend was listed twice");

        assert!(view.rows[0].is_header);
        assert_eq!(view.rows[0].header_label, "PINNED");
        assert_eq!(view.rows[1].id, "1");
    }

    #[test]
    fn collapsed_offline_group_keeps_its_header_but_drops_its_rows() {
        let friends = vec![friend(1, "Ghost", 0), friend(2, "Live", 1)];
        let view = friend_rows(&friends, &empty(), &empty(), "", true);

        assert!(view.rows.iter().any(|r| r.is_header && r.header_label == "OFFLINE"));
        assert!(
            !view.rows.iter().any(|r| !r.is_header && r.id == "1"),
            "collapsed group must not render its rows"
        );
        assert!(view.rows.iter().any(|r| !r.is_header && r.id == "2"));
    }

    #[test]
    fn filter_matches_display_name_and_username_case_insensitively() {
        let friends = vec![friend(1, "Alice", 1), friend(2, "Bob", 1)];

        let view = friend_rows(&friends, &empty(), &empty(), "ALI", false);
        let ids: Vec<String> = view.rows.iter().filter(|r| !r.is_header).map(|r| r.id.to_string()).collect();
        assert_eq!(ids, vec!["1"]);

        assert_eq!(view.online, 2);
    }

    #[test]
    fn online_count_includes_in_studio_like_the_group_does() {
        let friends = vec![friend(1, "Web", 1), friend(2, "Studio", 3)];
        let view = friend_rows(&friends, &empty(), &empty(), "", false);
        assert_eq!(view.online, 2);
    }

    #[test]
    fn empty_groups_emit_no_header() {
        let friends = vec![friend(1, "OnlyOnline", 1)];
        let view = friend_rows(&friends, &empty(), &empty(), "", false);
        assert!(!view.rows.iter().any(|r| r.is_header && r.header_label == "IN GAME"));
        assert!(!view.rows.iter().any(|r| r.is_header && r.header_label == "OFFLINE"));
    }

    #[test]
    fn subtitles_describe_what_the_friend_is_doing() {
        assert_eq!(subtitle_for(&friend(1, "A", 2)), "Playing Jailbreak");
        assert_eq!(subtitle_for(&friend(1, "A", 1)), "Online");
        assert_eq!(subtitle_for(&friend(1, "A", 3)), "In Studio");
        assert_eq!(subtitle_for(&friend(1, "A", 0)), "Offline");
    }

    #[test]
    fn in_game_with_no_location_still_reads_sensibly() {
        let mut f = friend(1, "A", 2);
        f.location = String::new();
        assert_eq!(subtitle_for(&f), "In game");
    }

    #[test]
    fn time_ago_is_empty_for_garbage_and_scales_otherwise() {
        assert_eq!(time_ago("nonsense"), "");
        assert_eq!(time_ago(""), "");

        let recent = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(time_ago(&recent), "5m ago");

        let older = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
        assert_eq!(time_ago(&older), "3h ago");

        let days = (chrono::Utc::now() - chrono::Duration::days(4)).to_rfc3339();
        assert_eq!(time_ago(&days), "4d ago");
    }

    #[test]
    fn a_future_timestamp_does_not_produce_negative_time() {
        let future = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        assert_eq!(time_ago(&future), "just now");
    }
}

// ---------------------------------------------------------------- graph ---

/// Everything the playtime chart needs, already flattened for Slint.
pub struct GraphModel {
    pub segments: Vec<crate::GraphSegment>,
    pub days: Vec<crate::GraphDay>,
    pub legend: Vec<crate::GraphLegend>,
    pub ceiling: String,
    pub total: String,
    pub range: String,
    pub empty: bool,
}

/// How many distinct tints the palette has before it starts reusing the last.
const TINTS: usize = 6;

/// Build the chart from raw sessions.
///
/// The tint a game gets comes from its rank across the *whole* window, not from
/// its rank within each day. That is the difference between a chart you can read
/// and a kaleidoscope: a game keeps one colour from one column to the next, so
/// the eye can follow it.
pub fn build_graph(
    sessions: &[rojoin_store::playtime::PlaySession],
    days: u32,
    now: i64,
) -> GraphModel {
    use rojoin_store::playtime;

    let buckets = playtime::daily(sessions, days, now);
    let ceiling = playtime::axis_ceiling_secs(&buckets);
    let overall = playtime::totals(sessions);

    // universe -> palette step, by overall rank.
    let tint_of: std::collections::HashMap<i64, i32> = overall
        .iter()
        .enumerate()
        .map(|(i, g)| (g.universe_id, i.min(TINTS - 1) as i32))
        .collect();

    let mut segments = Vec::new();
    for (day, bucket) in buckets.iter().enumerate() {
        // Stack from the baseline up, biggest first, so the heaviest band sits
        // on the axis where it is easiest to compare between columns.
        let mut start = 0.0_f32;
        let drawn = bucket.games.iter().filter(|g| g.secs > 0).count();
        let mut placed = 0usize;
        for g in &bucket.games {
            if g.secs == 0 {
                continue;
            }
            placed += 1;
            let size = g.secs as f32 / ceiling as f32;
            segments.push(crate::GraphSegment {
                day: day as i32,
                start,
                size,
                tint: tint_of.get(&g.universe_id).copied().unwrap_or(TINTS as i32 - 1),
                name: game_label(&g.name).into(),
                label: fmt_duration(g.secs).into(),
                day_label: bucket.label.clone().into(),
                top: placed == drawn,
            });
            start += size;
        }
    }

    let day_rows: Vec<crate::GraphDay> = buckets
        .iter()
        .map(|b| crate::GraphDay {
            label: b.label.clone().into(),
            total: fmt_duration(b.total_secs).into(),
            played: b.total_secs > 0,
        })
        .collect();

    let legend: Vec<crate::GraphLegend> = overall
        .iter()
        .filter(|g| g.secs > 0)
        .take(TINTS)
        .map(|g| crate::GraphLegend {
            name: game_label(&g.name).into(),
            label: fmt_duration(g.secs).into(),
            tint: tint_of.get(&g.universe_id).copied().unwrap_or(0),
        })
        .collect();

    let played: u64 = buckets.iter().map(|b| b.total_secs).sum();

    GraphModel {
        segments,
        days: day_rows,
        legend,
        ceiling: fmt_duration(ceiling).into(),
        total: format!("{} total", fmt_duration(played)),
        range: format!("last {days} days"),
        empty: played == 0,
    }
}

/// A name for a game the platform would not identify.
fn game_label(name: &str) -> String {
    if name.trim().is_empty() {
        "Hidden game".into()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;
    use rojoin_store::playtime::PlaySession;

    fn session(universe: i64, name: &str, start: i64, secs: i64) -> PlaySession {
        PlaySession {
            universe_id: universe,
            root_place_id: universe * 10,
            name: name.into(),
            start,
            end: start + secs,
        }
    }

    /// Midday, so a session in the same day cannot spill over a boundary.
    fn now() -> i64 {
        1_700_000_000
    }

    #[test]
    fn an_empty_history_reports_empty() {
        let g = build_graph(&[], 7, now());
        assert!(g.empty);
        assert!(g.segments.is_empty());
        assert_eq!(g.days.len(), 7, "the axis still spans the window");
    }

    #[test]
    fn segments_stack_without_overflowing_the_ceiling() {
        let g = build_graph(
            &[
                session(1, "Alpha", now() - 7200, 3600),
                session(2, "Beta", now() - 3600, 1800),
            ],
            3,
            now(),
        );
        assert!(!g.empty);
        // Everything on one day, so the stack tops out at start+size <= 1.
        let top = g
            .segments
            .iter()
            .map(|s| s.start + s.size)
            .fold(0.0_f32, f32::max);
        assert!(top <= 1.0001, "stack overflowed the plot: {top}");
        assert!(top > 0.0);
    }

    #[test]
    fn a_game_keeps_one_tint_across_days() {
        let g = build_graph(
            &[
                session(1, "Alpha", now() - 86_400 - 3600, 3600),
                session(2, "Beta", now() - 86_400 - 7200, 600),
                session(1, "Alpha", now() - 3600, 1800),
            ],
            3,
            now(),
        );
        let alpha: Vec<i32> = g
            .segments
            .iter()
            .filter(|s| s.name == "Alpha")
            .map(|s| s.tint)
            .collect();
        assert!(alpha.len() >= 2, "Alpha should appear on two days");
        assert!(
            alpha.iter().all(|t| *t == alpha[0]),
            "one game, one colour: {alpha:?}"
        );
    }

    #[test]
    fn the_biggest_game_overall_gets_the_first_tint() {
        let g = build_graph(
            &[
                session(2, "Small", now() - 7200, 600),
                session(1, "Big", now() - 3600, 7200),
            ],
            2,
            now(),
        );
        let big = g.segments.iter().find(|s| s.name == "Big").unwrap();
        assert_eq!(big.tint, 0);
        assert_eq!(g.legend[0].name, "Big");
    }

    #[test]
    fn more_games_than_tints_reuse_the_last_step() {
        let sessions: Vec<PlaySession> = (1..=9)
            .map(|i| session(i, &format!("G{i}"), now() - i * 600, 600 - i))
            .collect();
        let g = build_graph(&sessions, 2, now());
        assert!(g.segments.iter().all(|s| s.tint < TINTS as i32));
        assert_eq!(g.legend.len(), TINTS, "legend stops at the palette size");
    }

    #[test]
    fn a_hidden_game_is_labelled_rather_than_blank() {
        let mut s = session(0, "", now() - 3600, 1800);
        s.name = String::new();
        let g = build_graph(&[s], 2, now());
        assert!(g.segments.iter().any(|s| s.name == "Hidden game"));
    }

    #[test]
    fn zero_length_sessions_do_not_become_invisible_segments() {
        let g = build_graph(&[session(1, "Alpha", now() - 60, 0)], 2, now());
        assert!(g.segments.is_empty(), "a zero-length slice is not drawn");
    }
}

#[cfg(test)]
mod graph_cap_tests {
    use super::*;
    use rojoin_store::playtime::PlaySession;

    fn session(universe: i64, name: &str, start: i64, secs: i64) -> PlaySession {
        PlaySession {
            universe_id: universe,
            root_place_id: universe * 10,
            name: name.into(),
            start,
            end: start + secs,
        }
    }

    #[test]
    fn exactly_one_slice_per_column_is_capped() {
        let now = 1_700_000_000;
        let g = build_graph(
            &[
                session(1, "Alpha", now - 7200, 3600),
                session(2, "Beta", now - 3600, 1800),
                session(3, "Gamma", now - 1800, 600),
            ],
            2,
            now,
        );
        for day in 0..2 {
            let caps = g
                .segments
                .iter()
                .filter(|s| s.day == day && s.top)
                .count();
            let any = g.segments.iter().any(|s| s.day == day);
            assert_eq!(caps, if any { 1 } else { 0 }, "day {day} caps");
        }
    }

    #[test]
    fn the_cap_is_the_highest_slice() {
        let now = 1_700_000_000;
        let g = build_graph(
            &[
                session(1, "Alpha", now - 7200, 3600),
                session(2, "Beta", now - 3600, 1800),
            ],
            1,
            now,
        );
        let top = g.segments.iter().find(|s| s.top).unwrap();
        let highest = g
            .segments
            .iter()
            .max_by(|a, b| (a.start + a.size).partial_cmp(&(b.start + b.size)).unwrap())
            .unwrap();
        assert_eq!(top.name, highest.name);
    }

    #[test]
    fn every_slice_knows_its_day_for_the_hover_readout() {
        let now = 1_700_000_000;
        let g = build_graph(&[session(1, "Alpha", now - 3600, 1800)], 3, now);
        assert!(g.segments.iter().all(|s| !s.day_label.is_empty()));
    }
}

#[cfg(test)]
mod time_ago_unix_tests {
    use super::*;

    #[test]
    fn scales_from_minutes_to_weeks() {
        let now = 1_700_000_000;
        assert_eq!(time_ago_unix(now - 10, now), "just now");
        assert_eq!(time_ago_unix(now - 600, now), "10m ago");
        assert_eq!(time_ago_unix(now - 7200, now), "2h ago");
        assert_eq!(time_ago_unix(now - 3 * 86_400, now), "3d ago");
        assert_eq!(time_ago_unix(now - 21 * 86_400, now), "3w ago");
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_produce_a_negative() {
        let now = 1_700_000_000;
        assert_eq!(time_ago_unix(now + 5000, now), "just now");
    }
}
