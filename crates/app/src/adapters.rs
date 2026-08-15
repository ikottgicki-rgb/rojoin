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

// --- formatters -------------------------------------------------------------

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

pub fn today_label() -> String {
    chrono::Local::now().format("%A, %-d %B").to_string()
}

// --- game tiles -------------------------------------------------------------

pub fn tile_from_detail(d: &GameDetail, votes: Option<&Votes>, favorited: bool) -> GameTile {
    GameTile {
        id: d.root_place_id.to_string().into(),
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
            subtitle: SharedString::default(),
            thumb: Image::default(),
            kind: 3,
        })
        .collect()
}

// --- model helpers ----------------------------------------------------------

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
        // The whole reason ids are strings: this would overflow Slint's int.
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
}
