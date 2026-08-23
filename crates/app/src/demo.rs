//! Debug-only fake data, for looking at signed-in screens without a session.
//!
//! Compiled out of release builds entirely (`#[cfg(debug_assertions)]` at the
//! `mod` site), so none of this can ship. Enabled with `ROJOIN_DEMO=1`.
//!
//! It exists so the headless screenshot harness can render every screen while
//! iterating on the design, instead of pointing a test build at the user's real
//! config and writing to it.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::adapters as ad;
use crate::{DetailItem, GameDetailData, GameTile, MainWindow, ServerRow, SubPlace};

pub fn enabled() -> bool {
    std::env::var("ROJOIN_DEMO").is_ok_and(|v| v == "1")
}

pub fn seed(ui: &MainWindow) {
    ui.set_signed_in(true);
    ui.set_account_name("adam".into());
    ui.set_account_avatar(swatch(7, 64, 64));
    ui.set_accounts_count(2);
    ui.set_friends_online(4);
    ui.set_friend_requests(2);

    let games = [
        ("Jailbreak", "Badimo", "13K", 88),
        ("Tower Defense Simulator", "Paradoxum Games", "38K", 91),
        ("Doors", "LSPLASH", "52K", 95),
        ("Phantom Forces", "StyLiS Studios", "6.1K", 93),
        ("Bee Swarm Simulator", "Onett", "19K", 90),
        ("Natural Disaster Survival", "Stickmasterluke", "12K", 92),
    ];

    let tiles: Vec<GameTile> = games
        .iter()
        .enumerate()
        .map(|(i, (name, creator, playing, rating))| GameTile {
            universe_id: Default::default(),
            id: format!("{}", 100000 + i).into(),
            name: (*name).into(),
            creator: (*creator).into(),
            playing: (*playing).into(),
            rating: *rating,
            thumb: swatch(i, 320, 180),
            favorited: i % 3 == 0,
        })
        .collect();

    ui.set_hero(tiles[0].clone());
    ui.set_has_hero(true);
    ui.set_recent(ad::model(tiles.clone()));
    ui.set_favorites(ad::model(tiles.iter().take(4).cloned().collect()));

    ui.set_game(GameDetailData {
        universe_id: "245662005".into(),
        root_place_id: "606849621".into(),
        name: "Jailbreak".into(),
        creator: "Badimo".into(),
        creator_id: "1032204".into(),
        creator_is_group: true,
        description: "Orchestrate a robbery or catch criminals. Team up with friends for even more fun.".into(),
        playing: "13K".into(),
        visits: "8.2B".into(),
        favorites: "12M".into(),
        max_players: "24".into(),
        created: "27 Apr 2017".into(),
        updated: "14 Aug 2026".into(),
        genre: "Town and City".into(),
        rating: 88,
        favorited: true,
        notify: false,
        icon: swatch(3, 256, 256),
        hero: swatch(1, 1024, 420),
    });

    ui.set_sub_places(ad::model(vec![
        SubPlace {
            id: "1".into(),
            name: "Jailbreak: Winter Update".into(),
            description: "Seasonal map with the new vehicles".into(),
        },
        SubPlace {
            id: "2".into(),
            name: "Jailbreak: Testing Realm".into(),
            description: "Early builds, unstable".into(),
        },
        SubPlace {
            id: "3".into(),
            name: "Jailbreak: Museum".into(),
            description: "".into(),
        },
    ]));

    ui.set_servers(ad::model(
        [(23, 24), (18, 24), (11, 24), (3, 24)]
            .iter()
            .enumerate()
            .map(|(i, (playing, max))| ServerRow {
                id: format!("srv-{i}").into(),
                playing: *playing,
                max: *max,
                label: format!("{playing} / {max}").into(),
                fill: *playing as f32 / *max as f32,
                p0: swatch(i + 20, 48, 48),
                p1: swatch(i + 21, 48, 48),
                p2: swatch(i + 22, 48, 48),
                p3: swatch(i + 23, 48, 48),
                more: (*playing - 4).max(0),
            })
            .collect(),
    ));

    ui.set_passes(ad::model(
        (0..4)
            .map(|i| DetailItem {
                id: format!("p{i}").into(),
                name: format!("VIP Pass {}", i + 1).into(),
                subtitle: "".into(),
                thumb: swatch(i + 40, 128, 128),
                kind: 2,
            })
            .collect(),
    ));

    ui.set_related(ad::model(tiles.iter().skip(2).cloned().collect()));
    ui.set_play_games(ad::model(tiles.clone()));
    ui.set_play_last_query("jailbreak".into());
    ui.set_play_recents(ad::strings(vec![
        "jailbreak".into(),
        "tower defense".into(),
        "doors".into(),
    ]));

    seed_friends(ui);

    seed_profile(ui);
    seed_graph(ui);
    seed_group(ui);

    // ROJOIN_REQUESTS=1 opens the requests panel so it can be rendered.
    if std::env::var("ROJOIN_REQUESTS").is_ok_and(|v| v == "1") {
        ui.set_requests_open(true);
    }

    if let Some(n) = env_int("ROJOIN_SECTION") {
        ui.set_section(n);
    }
    if let Some(n) = env_int("ROJOIN_VIEW") {
        ui.set_view_kind(n);
        ui.set_can_back(n != 0);
    }
}

fn env_int(key: &str) -> Option<i32> {
    std::env::var(key).ok()?.parse().ok()
}

fn seed_friends(ui: &MainWindow) {
    let people: &[(i64, &str, &str, i32, &str, bool, bool)] = &[
        (1, "UsedHenry06", "usedhenry06", 2, "Jailbreak", true, true),
        (2, "felix", "felixsson", 2, "Tower Defense Simulator", true, false),
        (3, "quuut", "quuut", 1, "", false, false),
        (4, "Floofy", "floofyiv", 1, "", false, true),
        (5, "Nova", "novabuilds", 3, "", false, false),
        (6, "Marcus", "marcusdev", 0, "", false, false),
        (7, "Ellie", "elliebuilds", 0, "", false, false),
        (8, "Tom", "tomtomtom", 0, "", false, false),
    ];

    let inputs: Vec<crate::adapters::FriendInput> = people
        .iter()
        .map(|(id, name, user, presence, loc, _, _)| crate::adapters::FriendInput {
            id: *id,
            name: (*name).into(),
            username: (*user).into(),
            presence: *presence,
            location: (*loc).into(),
            last_online: (chrono::Utc::now() - chrono::Duration::hours(*id * 3)).to_rfc3339(),
            place_id: (*presence == 2).then_some(606849621),
            game_id: None,
            avatar_url: String::new(),
            joinable: *presence == 2,
        })
        .collect();

    let pinned: std::collections::HashSet<String> = people
        .iter()
        .filter(|p| p.5)
        .map(|p| p.0.to_string())
        .collect();
    let notify: std::collections::HashSet<String> = people
        .iter()
        .filter(|p| p.6)
        .map(|p| p.0.to_string())
        .collect();

    let view = ad::friend_rows(&inputs, &pinned, &notify, "", false);
    ui.set_friends_in_game(view.in_game);
    ui.set_friends_online(view.online);

    let rows: Vec<crate::FriendRow> = view
        .rows
        .into_iter()
        .map(|mut r| {
            if !r.is_header {
                let seed = r.id.parse::<usize>().unwrap_or(0) + 60;
                r.avatar = swatch(seed, 72, 72);
            }
            r
        })
        .collect();
    ui.set_friend_rows(ad::model(rows));

    let requests: &[(&str, &str, &str, &str, i32, &str)] = &[
        ("101", "NewPerson", "newperson", "Builder. Mostly obbies.", 2, "Playing Doors"),
        ("102", "SomeoneElse", "someoneelse", "", 1, "Online"),
        ("103", "Quiet", "quietone", "Just here for the trains.", 0, ""),
    ];
    ui.set_requests_list(ad::model(
        requests
            .iter()
            .enumerate()
            .map(|(i, (id, name, username, about, presence, label))| crate::RequestRow {
                id: (*id).into(),
                name: (*name).into(),
                username: (*username).into(),
                description: (*about).into(),
                presence: *presence,
                presence_label: (*label).into(),
                thumb: swatch(i + 90, 72, 72),
            })
            .collect(),
    ));
}

/// A flat placeholder image so layout is exercised with real decoded pixels
/// rather than an empty `Image` hiding sizing bugs behind a background colour.
fn swatch(seed: usize, w: u32, h: u32) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let width = buf.width();
    let pixels = buf.make_mut_slice();

    let s = seed.wrapping_mul(2_654_435_761);
    let base = 24 + (s % 30) as u8;

    for (i, px) in pixels.iter_mut().enumerate() {
        let x = (i as u32 % width) as f32 / w as f32;
        let y = (i as u32 / width) as f32 / h as f32;
        let v = (base as f32 + (x * 0.6 + y * 0.4) * 26.0).clamp(0.0, 255.0) as u8;
        *px = Rgba8Pixel { r: v, g: v, b: v.saturating_add(6), a: 255 };
    }

    Image::from_rgba8(buf)
}

/// A profile to look at. `ROJOIN_ME=1` makes it your own, which is what mounts
/// the editable About me.
fn seed_profile(ui: &MainWindow) {
    let me = std::env::var("ROJOIN_ME").is_ok_and(|v| v == "1");

    ui.set_profile_is_me(me);
    ui.set_profile(crate::ProfileData {
        id: "9314226124".into(),
        name: if me { "adam".into() } else { "UsedHenry06".into() },
        username: if me { "adam".into() } else { "usedhenry06".into() },
        description: "Building things in Studio, mostly. Ask me about trains."
            .into(),
        joined: "3 years".into(),
        also_known_as: "".into(),
        verified: false,
        friends: "48".into(),
        followers: "126".into(),
        following: "83".into(),
        presence: 2,
        presence_label: "In Jailbreak".into(),
        place_id: "606849621".into(),
        is_friend: !me,
        has_incoming_request: false,
        is_self: me,
        avatar: swatch(3, 96, 96),
    });

    // Someone else's profile gets the tracked-playtime tab; your own does not,
    // since we only sample pinned friends.
    if !me {
        seed_profile_stats(ui);
    }
}

fn seed_profile_stats(ui: &MainWindow) {
    use rojoin_store::playtime;

    let now = chrono::Utc::now().timestamp();
    let sessions = fake_sessions(now, 11);
    let g = ad::build_graph(&sessions, 14, now);

    ui.set_profile_has_stats(true);
    ui.set_profile_graph_segments(ad::model(g.segments));
    ui.set_profile_graph_days(ad::model(g.days));
    ui.set_profile_graph_legend(ad::model(g.legend));
    ui.set_profile_graph_ceiling(g.ceiling.into());
    ui.set_profile_graph_total(g.total.into());
    ui.set_profile_graph_range(g.range.into());
    ui.set_profile_graph_empty(g.empty);

    let totals = playtime::totals(&sessions);
    let top = totals.first().map(|t| t.secs).unwrap_or(1);
    ui.set_profile_most_played(ad::model(
        totals
            .iter()
            .take(5)
            .map(|t| crate::PlayStat {
                id: t.root_place_id.to_string().into(),
                name: if t.name.is_empty() { "Hidden game".into() } else { t.name.clone().into() },
                value: ad::fmt_duration(t.secs).into(),
                fraction: t.secs as f32 / top as f32,
            })
            .collect(),
    ));

    let tracked: u64 = sessions.iter().map(|s| s.secs()).sum();
    ui.set_profile_stat_total(ad::fmt_duration(tracked).into());
    ui.set_profile_stat_longest(
        playtime::longest(&sessions).map(|s| ad::fmt_duration(s.secs())).unwrap_or_default().into(),
    );
    ui.set_profile_stat_sessions(sessions.len().to_string().into());
    ui.set_profile_stat_since("14d".into());
    ui.set_profile_recent_sessions(ad::model(
        playtime::latest(&sessions, 6)
            .into_iter()
            .map(|s| crate::DetailItem {
                id: s.root_place_id.to_string().into(),
                name: if s.name.is_empty() { "Hidden game".into() } else { s.name.clone().into() },
                subtitle: format!(
                    "{} · {}",
                    ad::time_ago_unix(s.end, now),
                    ad::fmt_duration(s.secs())
                )
                .into(),
                thumb: slint::Image::default(),
                kind: 1,
            })
            .collect(),
    ));
    ui.set_profile_tab(1);
}

/// A fortnight of plausible sessions, so the chart can be looked at.
fn fake_sessions(now: i64, spread: i64) -> Vec<rojoin_store::playtime::PlaySession> {
    use rojoin_store::playtime::PlaySession;

    let games = [
        (245662005_i64, "Jailbreak"),
        (1962086868, "Tower Defense Simulator"),
        (2440500124, "Doors"),
        (0, ""),
    ];

    let mut sessions = Vec::new();
    for day in 0..14_i64 {
        let base = now - day * 86_400 - 6 * 3600;
        let n = ((day * spread) % 3) + 1;
        for k in 0..n {
            let (universe, name) = games[((day + k + spread) % 4) as usize];
            let start = base + k * 5400;
            let secs = 900 + ((day * 13 + k * 29 + spread) % 47) * 120;
            sessions.push(PlaySession {
                universe_id: universe,
                root_place_id: universe * 10,
                name: name.to_string(),
                start,
                end: start + secs,
            });
        }
    }
    sessions
}

fn seed_graph(ui: &MainWindow) {
    let now = chrono::Utc::now().timestamp();
    // Deterministic but uneven, so the columns do not all look the same.
    let sessions = fake_sessions(now, 7);
    let g = ad::build_graph(&sessions, 14, now);
    ui.set_show_graph(true);
    ui.set_graph_segments(ad::model(g.segments));
    ui.set_graph_days(ad::model(g.days));
    ui.set_graph_legend(ad::model(g.legend));
    ui.set_graph_ceiling(g.ceiling.into());
    ui.set_graph_total(g.total.into());
    ui.set_graph_range(g.range.into());
    ui.set_graph_empty(g.empty);
}

/// A group with members, so the rank list can be rendered headless.
fn seed_group(ui: &MainWindow) {
    let people: &[(&str, &str, &str, &str, i32)] = &[
        ("1", "Badimo", "badimo", "Owner", 255),
        ("2", "asimo3089", "asimo3089", "Developer", 200),
        ("3", "bad_cc", "badcc", "Developer", 200),
        ("4", "Helper One", "helperone", "Moderator", 100),
        ("5", "Helper Two", "helpertwo", "Moderator", 100),
        ("6", "Someone", "someone", "Member", 1),
    ];
    ui.set_group_member_total(ad::compact(184_302).into());
    ui.set_group_members(ad::model(
        people
            .iter()
            .enumerate()
            .map(|(i, (id, name, user, role, rank))| crate::MemberRow {
                id: (*id).into(),
                name: (*name).into(),
                username: (*user).into(),
                role: (*role).into(),
                rank: *rank,
                thumb: swatch(i + 40, 64, 64),
            })
            .collect(),
    ));
}
