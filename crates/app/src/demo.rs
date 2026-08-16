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

    ui.set_requests_list(ad::model(
        [("101", "NewPerson"), ("102", "SomeoneElse")]
            .iter()
            .enumerate()
            .map(|(i, (id, name))| DetailItem {
                id: (*id).into(),
                name: (*name).into(),
                subtitle: "".into(),
                thumb: swatch(i + 90, 72, 72),
                kind: 8,
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
