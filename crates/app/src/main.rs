// No console window on Windows. This single line is the difference between
// shipping "a .exe" and shipping "a .exe that flashes a black box on launch".
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};

use rojoin_launcher::JoinRequest;
use rojoin_roblox::{auth, games, search, thumbnails, users, Client};
use rojoin_store::Config;
use slint::ComponentHandle;

slint::include_modules!();

mod adapters;
mod bridge;
#[cfg(debug_assertions)]
mod demo;
mod images;
mod secrets;

use adapters as ad;
use bridge::Bridge;
use images::Images;

/// Bumped on every account switch and sign-in. Any in-flight load whose
/// generation is stale drops its result instead of writing one account's data
/// into another account's screen.
static SESSION_GEN: AtomicI64 = AtomicI64::new(0);

/// Bumped per search. Guards against a slow earlier query landing after a
/// faster later one and repopulating results the user has moved past.
static SEARCH_GEN: AtomicI64 = AtomicI64::new(0);

/// Sidebar order. The index is the `section` property, so this is the single
/// source of truth for navigation.
const NAV: &[(&str, &str)] = &[
    ("Home", "⌂"),
    ("Play", "▷"),
    ("Friends", "◑"),
    ("Chat", "✉"),
    ("Library", "▤"),
    ("Avatar", "☺"),
    ("Macros", "⌨"),
];

/// UI-thread state. Everything here is touched only from the Slint event loop.
struct App {
    client: Client,
    config: Mutex<Config>,
    /// The universe currently open in the detail view, for follow-up fetches.
    current_universe: Mutex<i64>,
    current_place: Mutex<i64>,
    /// Live quick-login attempt, if any.
    signin: Mutex<Option<auth::LoginCode>>,
    /// Section to return to when a pushed detail view is dismissed.
    return_section: Mutex<i32>,
    search_session: Mutex<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rojoin=info,rojoin_v4=info".into()),
        )
        .init();

    let ui = MainWindow::new()?;
    let bridge = Arc::new(Bridge::new(&ui)?);
    let client = Client::new()?;
    let imgs = Images::new(client.clone(), ui.as_weak(), bridge.runtime().clone());

    let app = Arc::new(App {
        client: client.clone(),
        config: Mutex::new(Config::load()),
        current_universe: Mutex::new(0),
        current_place: Mutex::new(0),
        signin: Mutex::new(None),
        return_section: Mutex::new(0),
        search_session: Mutex::new(new_session_id()),
    });

    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.set_nav(ad::model(
        NAV.iter()
            .enumerate()
            .map(|(i, (label, glyph))| NavEntry {
                id: i as i32,
                label: (*label).into(),
                glyph: (*glyph).into(),
                badge: 0,
            })
            .collect(),
    ));
    ui.set_date_label(ad::today_label().into());
    ui.set_greeting(ad::greeting(chrono::Local::now().format("%H").to_string().parse().unwrap_or(12)).into());

    wire_signin(&ui, &app, &bridge, &imgs);
    wire_nav(&ui, &app);
    wire_search(&ui, &app, &bridge, &imgs);
    wire_game(&ui, &app, &bridge, &imgs);
    wire_launch(&ui, &app, &bridge);

    #[cfg(debug_assertions)]
    let demo_mode = demo::enabled();
    #[cfg(not(debug_assertions))]
    let demo_mode = false;

    if demo_mode {
        #[cfg(debug_assertions)]
        demo::seed(&ui);
    } else {
        restore_session(&ui, &app, &bridge, &imgs);
    }

    ui.run()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Try the stored cookie. A failure here is not an error the user should see —
/// it just means the sign-in screen instead of the app.
fn restore_session(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let active = app.config.lock().unwrap().active_account.clone();
    let Some(id) = active else { return };
    let Some(cookie) = secrets::load(&id) else {
        tracing::info!(account = %id, "no stored cookie for the active account");
        return;
    };

    let client = app.client.clone();
    let app = app.clone();
    let bridge2 = bridge.clone();
    let imgs = imgs.clone();
    let weak = ui.as_weak();

    bridge.spawn(async move {
        client.set_cookie(Some(cookie)).await;
        let me = users::authenticated(&client).await;

        let _ = weak.upgrade_in_event_loop(move |ui| match me {
            Ok(me) => {
                tracing::info!(user = %me.name, "restored session");
                enter_app(&ui, &app, &bridge2, &imgs, &me.display_name, me.id);
            }
            Err(e) => {
                tracing::info!(error = %e, "stored session is no longer valid");
                ui.set_signed_in(false);
            }
        });
    });
}

fn enter_app(
    ui: &MainWindow,
    app: &Arc<App>,
    bridge: &Arc<Bridge>,
    imgs: &Images,
    display_name: &str,
    user_id: i64,
) {
    SESSION_GEN.fetch_add(1, Ordering::SeqCst);

    ui.set_signed_in(true);
    ui.set_session_expired(false);
    ui.set_account_name(display_name.into());
    ui.set_accounts_count(app.config.lock().unwrap().accounts.len() as i32);

    // The account's own head shot in the top bar.
    let imgs2 = imgs.clone();
    let client = app.client.clone();
    bridge.spawn(async move {
        if let Ok(map) = thumbnails::headshots(&client, &[user_id]).await {
            if let Some(url) = map.get(&user_id).cloned() {
                imgs2.load(&url, move |ui, img| ui.set_account_avatar(img));
            }
        }
    });

    load_home(ui, app, bridge, imgs);
}

// ---------------------------------------------------------------------------
// Sign-in (cross-device quick login)
// ---------------------------------------------------------------------------

fn wire_signin(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs = imgs.clone();
        let weak = ui.as_weak();

        ui.on_signin_start(move || {
            let ui = weak.unwrap();
            ui.set_signin_phase(SignInPhase::Requesting);
            ui.set_signin_error("".into());

            let client = app.client.clone();
            let app = app.clone();
            let bridge3 = bridge2.clone();
            let imgs = imgs.clone();
            let weak2 = weak.clone();

            bridge2.spawn(async move {
                match auth::create(client.raw()).await {
                    Ok(code) => {
                        let display = code.display_code();
                        let qr = code.qr_url();
                        let c2 = code.clone();

                        let imgs2 = imgs.clone();
                        let _ = weak2.upgrade_in_event_loop(move |ui| {
                            ui.set_signin_code(display.into());
                            ui.set_signin_phase(SignInPhase::Waiting);
                            imgs2.load(&qr, |ui, img| ui.set_signin_qr(img));
                        });

                        *app.signin.lock().unwrap() = Some(c2.clone());
                        poll_until_approved(client, app, bridge3, imgs, weak2, c2).await;
                    }
                    Err(e) => {
                        let _ = weak2.upgrade_in_event_loop(move |ui| {
                            ui.set_signin_phase(SignInPhase::Failed);
                            ui.set_signin_error(
                                format!("Could not reach Roblox: {e}").into(),
                            );
                        });
                    }
                }
            });
        });
    }

    {
        let app = app.clone();
        ui.on_signin_open_browser(move || {
            // The user's own default browser, where they are already signed in.
            // Nothing is embedded and no password passes through this app.
            let url = app
                .signin
                .lock().unwrap()
                .as_ref()
                .map(auth::LoginCode::confirm_url)
                .unwrap_or_else(|| "https://www.roblox.com/crossdevicelogin/ConfirmCode".into());

            if let Err(e) = webbrowser::open(&url) {
                tracing::error!(error = %e, "could not open the browser");
            }
        });
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_signin_cancel(move || {
            app.signin.lock().unwrap().take();
            let ui = weak.unwrap();
            ui.set_signin_phase(SignInPhase::Idle);
            ui.set_signin_code("".into());
        });
    }

    {
        let weak = ui.as_weak();
        ui.on_signin_back(move || {
            weak.unwrap().set_signed_in(true);
        });
    }

    {
        let weak = ui.as_weak();
        ui.on_dismiss_expired(move || weak.unwrap().set_session_expired(false));
    }
}

/// Poll Roblox until the user approves in their browser, then redeem the code
/// for a cookie.
async fn poll_until_approved(
    client: Client,
    app: Arc<App>,
    bridge: Arc<Bridge>,
    imgs: Images,
    weak: slint::Weak<MainWindow>,
    code: auth::LoginCode,
) {
    let mut csrf: Option<String> = None;
    let mut elapsed = std::time::Duration::ZERO;
    // Roblox codes last a few minutes; stop well after that rather than
    // polling a dead code forever.
    let budget = std::time::Duration::from_secs(360);

    loop {
        tokio::time::sleep(auth::POLL_INTERVAL).await;
        elapsed += auth::POLL_INTERVAL;

        if elapsed > budget {
            let _ = weak.upgrade_in_event_loop(|ui| {
                ui.set_signin_phase(SignInPhase::Failed);
                ui.set_signin_error("That code expired. Get a new one to try again.".into());
            });
            return;
        }

        match auth::poll(client.raw(), &code, &mut csrf).await {
            Ok((auth::LoginStatus::Pending, _)) => continue,

            Ok((auth::LoginStatus::Cancelled, _)) => {
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.set_signin_phase(SignInPhase::Failed);
                    ui.set_signin_error("That sign-in was declined.".into());
                });
                return;
            }

            Ok((auth::LoginStatus::Validated, account_name)) => {
                match auth::redeem(client.raw(), &code, account_name, &mut csrf).await {
                    Ok(session) => {
                        finish_signin(client, app, bridge, imgs, weak, session).await;
                        return;
                    }
                    Err(e) => {
                        let _ = weak.upgrade_in_event_loop(move |ui| {
                            ui.set_signin_phase(SignInPhase::Failed);
                            ui.set_signin_error(
                                format!("Approved, but the sign-in did not complete: {e}").into(),
                            );
                        });
                        return;
                    }
                }
            }

            // A transient poll failure is not fatal — keep waiting.
            Err(e) => {
                tracing::debug!(error = %e, "sign-in poll hiccup");
                continue;
            }
        }
    }
}

async fn finish_signin(
    client: Client,
    app: Arc<App>,
    bridge: Arc<Bridge>,
    imgs: Images,
    weak: slint::Weak<MainWindow>,
    session: auth::Session,
) {
    client.set_cookie(Some(session.cookie.clone())).await;

    let me = match users::authenticated(&client).await {
        Ok(me) => me,
        Err(e) => {
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_signin_phase(SignInPhase::Failed);
                ui.set_signin_error(format!("Signed in, but Roblox rejected the session: {e}").into());
            });
            return;
        }
    };

    let _ = weak.upgrade_in_event_loop(move |ui| {
        if let Err(e) = secrets::store(&me.id.to_string(), &session.cookie) {
            tracing::error!(error = %e, "could not persist the cookie");
        }

        {
            let mut cfg = app.config.lock().unwrap();
            cfg.upsert_account(rojoin_store::Account {
                id: me.id.to_string(),
                username: me.name.clone(),
                display_name: me.display_name.clone(),
                avatar_url: String::new(),
            });
            cfg.active_account = Some(me.id.to_string());
            if let Err(e) = cfg.save() {
                tracing::error!(error = %e, "could not save config");
            }
        }

        ui.set_signin_phase(SignInPhase::Approved);
        ui.set_account_name(me.display_name.clone().into());
        app.signin.lock().unwrap().take();

        enter_app(&ui, &app, &bridge, &imgs, &me.display_name, me.id);
    });
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

fn wire_nav(ui: &MainWindow, app: &Arc<App>) {
    {
        let weak = ui.as_weak();
        ui.on_navigate(move |i| {
            let ui = weak.unwrap();
            ui.set_section(i);
            ui.set_view_kind(0);
            ui.set_can_back(false);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_nav_back(move || {
            let ui = weak.unwrap();
            if ui.get_view_kind() != 0 {
                ui.set_view_kind(0);
                ui.set_section(*app.return_section.lock().unwrap());
                ui.set_can_back(false);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_open_settings(move || weak.unwrap().set_section(NAV.len() as i32 - 1));
    }
    ui.on_open_account(|| tracing::info!("account switcher: milestone 2"));
    ui.on_open_creator(|| tracing::info!("creator profile: milestone 2"));
    ui.on_open_player(|id| tracing::info!(%id, "profile: milestone 2"));
    ui.on_open_group(|id| tracing::info!(%id, "group: milestone 2"));
    ui.on_toggle_notify(|| tracing::info!("notify: milestone 2"));
    ui.on_copy_link(|| tracing::info!("copy link: milestone 6"));
    ui.on_open_browser(|| tracing::info!("open in browser: milestone 6"));
}

// ---------------------------------------------------------------------------
// Home
// ---------------------------------------------------------------------------

fn load_home(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let history: Vec<(i64, i64)> = {
        let cfg = app.config.lock().unwrap();
        let Some(data) = cfg.data() else {
            ui.set_home_loading(false);
            return;
        };
        let mut items: Vec<_> = data.history.values().collect();
        items.sort_by_key(|h| std::cmp::Reverse(h.last_played));
        items
            .iter()
            .filter_map(|h| h.place_id.parse::<i64>().ok().map(|p| (p, h.last_played)))
            .take(12)
            .collect()
    };

    if history.is_empty() {
        ui.set_home_loading(false);
        ui.set_has_hero(false);
        return;
    }

    ui.set_home_loading(true);

    let client = app.client.clone();
    let imgs = imgs.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);
    let place_ids: Vec<i64> = history.iter().map(|(p, _)| *p).collect();

    bridge.call_res(
        move || async move { fetch_home(&client, &place_ids).await },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_home_loading(false);

            let load = match result {
                Ok(load) => load,
                Err(e) => return bridge::report(&ui, e),
            };

            // Tiles are built here, on the UI thread, because `slint::Image`
            // is not Send and so cannot be constructed inside the async task.
            let tiles: Vec<GameTile> = load
                .details
                .iter()
                .map(|d| {
                    let v = load.votes.iter().find(|v| v.id == d.id);
                    ad::tile_from_detail(d, v, false)
                })
                .collect();

            if let Some(first) = tiles.first().cloned() {
                ui.set_hero(first);
                ui.set_has_hero(true);
            }
            ui.set_recent(ad::model(tiles));

            for (i, d) in load.details.iter().enumerate() {
                let Some(url) = load.art.get(&d.id).cloned() else { continue };
                imgs.load(&url, move |ui, img| {
                    set_tile_thumb(&ui.get_recent(), i, img)
                });
                if i == 0 {
                    imgs.load(&url, |ui, img| {
                        let mut h = ui.get_hero();
                        h.thumb = img;
                        ui.set_hero(h);
                    });
                }
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

fn wire_search(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_play_search(move |q| {
            run_search(&weak.unwrap(), &app, &bridge2, &imgs2, q.as_str());
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_top_submit(move |q| {
            let ui = weak.unwrap();
            // A pasted link or a long bare id jumps straight to the game.
            if let Some(place_id) = search::resolve_place_id(q.as_str()) {
                open_game(&ui, &app, &bridge2, &imgs2, place_id);
                return;
            }
            ui.set_section(1);
            ui.set_view_kind(0);
            ui.set_play_query(q.clone());
            run_search(&ui, &app, &bridge2, &imgs2, q.as_str());
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_top_live_search(move |q| {
            // Clearing the box must also invalidate any in-flight response, or
            // a slow reply repopulates results the user just dismissed.
            if q.is_empty() {
                SEARCH_GEN.fetch_add(1, Ordering::SeqCst);
                let ui = weak.unwrap();
                ui.set_play_searching(false);
            }
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_clear_recents(move || {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.data_mut().recent_searches.clear();
                let _ = cfg.save();
            }
            ui.set_play_recents(ad::strings(Vec::new()));
        });
    }
}

fn run_search(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images, query: &str) {
    let query = query.trim().to_string();
    if query.is_empty() {
        SEARCH_GEN.fetch_add(1, Ordering::SeqCst);
        ui.set_play_searching(false);
        ui.set_play_last_query("".into());
        ui.set_play_games(ad::model(Vec::new()));
        return;
    }

    {
        let mut cfg = app.config.lock().unwrap();
        cfg.push_recent_search(&query);
        let _ = cfg.save();
        let recents = cfg.data().map(|d| d.recent_searches.clone()).unwrap_or_default();
        ui.set_play_recents(ad::strings(recents));
    }

    ui.set_play_searching(true);
    let gen = SEARCH_GEN.fetch_add(1, Ordering::SeqCst) + 1;

    let client = app.client.clone();
    let imgs = imgs.clone();
    let session = app.search_session.lock().unwrap().clone();
    let q2 = query.clone();

    bridge.call_res(
        move || async move {
            let page = search::games(&client, &q2, &session, None).await?;
            let ids: Vec<i64> = page.games.iter().map(|g| g.universe_id).collect();
            let art = thumbnails::game_art(&client, &ids).await.unwrap_or_default();
            Ok((page.games, art))
        },
        move |ui, result| {
            if gen != SEARCH_GEN.load(Ordering::SeqCst) {
                return; // a newer search superseded this one
            }
            ui.set_play_searching(false);
            ui.set_play_last_query(query.clone().into());

            match result {
                Ok((found, art)) => {
                    let tiles: Vec<GameTile> = found.iter().map(ad::tile_from_omni).collect();
                    ui.set_play_games(ad::model(tiles));

                    for (i, g) in found.iter().enumerate() {
                        if let Some(url) = art.get(&g.universe_id).cloned() {
                            imgs.load(&url, move |ui, img| {
                                set_tile_thumb(&ui.get_play_games(), i, img)
                            });
                        }
                    }
                }
                Err(e) => {
                    // Clear stale results so the screen never shows the last
                    // query's hits under this query's heading.
                    ui.set_play_games(ad::model(Vec::new()));
                    bridge::report(&ui, e);
                }
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Game detail
// ---------------------------------------------------------------------------

fn wire_game(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_open_game(move |id| {
            if let Ok(place_id) = id.parse::<i64>() {
                open_game(&weak.unwrap(), &app, &bridge2, &imgs2, place_id);
            }
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_set_server_sort(move |sort| {
            fetch_servers(&weak.unwrap(), &app, &bridge2, sort);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_toggle_favorite(move |universe_id| {
            let ui = weak.unwrap();
            let Ok(uid) = universe_id.parse::<i64>() else { return };

            // Optimistic: flip immediately, reconcile if Roblox disagrees.
            let mut g = ui.get_game();
            let target = !g.favorited;
            g.favorited = target;
            ui.set_game(g);

            let client = app.client.clone();
            bridge2.call_res(
                move || async move { games::set_favorited(&client, uid, target).await },
                move |ui, result| {
                    if result.is_err() {
                        let mut g = ui.get_game();
                        g.favorited = !target;
                        ui.set_game(g);
                        if let Err(e) = result {
                            bridge::report(&ui, e);
                        }
                    }
                },
            );
        });
    }
}

fn open_game(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images, place_id: i64) {
    if ui.get_view_kind() == 0 {
        *app.return_section.lock().unwrap() = ui.get_section();
    }
    ui.set_view_kind(1);
    ui.set_can_back(true);
    ui.set_game_loading(true);
    ui.set_game_tab(0);
    ui.set_sub_places(ad::model(Vec::new()));
    ui.set_servers(ad::model(Vec::new()));
    *app.current_place.lock().unwrap() = place_id;

    let client = app.client.clone();
    let imgs = imgs.clone();
    let app2 = app.clone();
    let bridge2 = bridge.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move {
            let universe_id = games::universe_of(&client, place_id).await?;
            let detail = games::detail(&client, universe_id).await?;
            let votes = games::votes(&client, &[universe_id]).await.unwrap_or_default();
            let subs = games::sub_places(&client, universe_id, detail.root_place_id)
                .await
                .unwrap_or_default();
            let passes = games::game_passes(&client, universe_id).await.unwrap_or_default();
            let badges = games::badges(&client, universe_id).await.unwrap_or_default();
            let icons = thumbnails::game_icons(&client, &[universe_id]).await.unwrap_or_default();
            let art = thumbnails::game_art(&client, &[universe_id]).await.unwrap_or_default();

            Ok(GameLoad {
                universe_id,
                detail,
                votes: votes.into_iter().next(),
                subs,
                passes,
                badges,
                icon: icons.get(&universe_id).cloned(),
                hero: art.get(&universe_id).cloned(),
            })
        },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_game_loading(false);

            match result {
                Ok(load) => {
                    *app2.current_universe.lock().unwrap() = load.universe_id;
                    ui.set_game(ad::detail_data(&load.detail, load.votes.as_ref(), false));
                    ui.set_sub_places(ad::model(ad::sub_places(&load.subs)));
                    ui.set_passes(ad::model(ad::passes(&load.passes)));
                    ui.set_badges(ad::model(ad::badges(&load.badges)));

                    if let Some(url) = load.icon {
                        imgs.load(&url, |ui, img| {
                            let mut g = ui.get_game();
                            g.icon = img;
                            ui.set_game(g);
                        });
                    }
                    if let Some(url) = load.hero {
                        imgs.load(&url, |ui, img| {
                            let mut g = ui.get_game();
                            g.hero = img;
                            ui.set_game(g);
                        });
                    }

                    fetch_servers(&ui, &app2, &bridge2, ui.get_server_sort());
                }
                Err(e) => bridge::report(&ui, e),
            }
        },
    );
}

struct GameLoad {
    universe_id: i64,
    detail: rojoin_roblox::models::GameDetail,
    votes: Option<rojoin_roblox::models::Votes>,
    subs: Vec<rojoin_roblox::models::Place>,
    passes: Vec<rojoin_roblox::models::GamePass>,
    badges: Vec<rojoin_roblox::models::Badge>,
    icon: Option<String>,
    hero: Option<String>,
}

fn fetch_servers(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, sort: i32) {
    // Servers key on placeId, never universeId — a universe id returns 400.
    let place_id = *app.current_place.lock().unwrap();
    if place_id == 0 {
        return;
    }

    ui.set_servers_loading(true);
    let client = app.client.clone();
    let sort = match sort {
        1 => games::ServerSort::Fullest,
        2 => games::ServerSort::Emptiest,
        _ => games::ServerSort::Default,
    };

    bridge.call_res(
        move || async move { games::servers(&client, place_id, 25, sort).await },
        move |ui, result| {
            ui.set_servers_loading(false);
            match result {
                Ok(list) => ui.set_servers(ad::model(ad::servers(&list))),
                Err(e) => {
                    ui.set_servers(ad::model(Vec::new()));
                    bridge::report(&ui, e);
                }
            }
        },
    );
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

fn wire_launch(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>) {
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_play_game(move |id| {
            let ui = weak.unwrap();
            if let Ok(place_id) = id.parse::<i64>() {
                launch(&ui, &app, JoinRequest::place(place_id));
            }
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_play_sub_place(move |id| {
            let ui = weak.unwrap();
            let Ok(place_id) = id.parse::<i64>() else { return };

            // Launch the chosen sub-place, but keep history and per-game
            // settings attributed to the game's root place.
            let root = ui.get_game().root_place_id.parse::<i64>().unwrap_or(place_id);
            launch(&ui, &app, JoinRequest::sub_place(place_id, root));
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_join_server(move |job_id| {
            let ui = weak.unwrap();
            let place_id = *app.current_place.lock().unwrap();
            if place_id != 0 {
                launch(&ui, &app, JoinRequest::place(place_id).server(job_id.as_str()));
            }
        });
    }
    let _ = bridge;
}

fn launch(ui: &MainWindow, app: &Arc<App>, req: JoinRequest) {
    ui.set_launching(true);

    let name = ui.get_game().name.to_string();
    {
        let mut cfg = app.config.lock().unwrap();
        let key = if req.root_place_id != 0 { req.root_place_id } else { req.place_id };
        cfg.record_launch(&key.to_string(), &name, chrono::Utc::now().timestamp());
        let _ = cfg.save();
    }

    let result = match rojoin_launcher::detect() {
        rojoin_launcher::Backend::Sober => rojoin_launcher::launch_sober(&req),
        rojoin_launcher::Backend::WindowsClient => {
            // Windows needs a fresh auth ticket per launch; wired in M1's
            // Windows pass once there is a machine to test it on.
            Err(rojoin_launcher::Error::Launch(
                "Windows launching is not wired up yet".into(),
            ))
        }
    };

    if let Err(e) = result {
        tracing::error!(error = %e, "launch failed");
    } else {
        tracing::info!(
            place = req.place_id,
            sub_place = req.is_sub_place(),
            "launched"
        );
    }

    // The launcher hands off to another process; there is nothing to await.
    ui.set_launching(false);
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Everything Home needs, in `Send`-safe form. No `slint::Image` here —
/// images are not `Send`, so the tile structs are assembled on the UI thread.
struct HomeLoad {
    details: Vec<rojoin_roblox::models::GameDetail>,
    votes: Vec<rojoin_roblox::models::Votes>,
    art: std::collections::HashMap<i64, String>,
}

async fn fetch_home(client: &Client, place_ids: &[i64]) -> rojoin_roblox::Result<HomeLoad> {
    // Resolve places to universes, dropping any that no longer resolve rather
    // than failing the whole screen because one game was taken down.
    let mut universes = Vec::new();
    for place in place_ids {
        if let Ok(u) = games::universe_of(client, *place).await {
            universes.push(u);
        }
    }

    Ok(HomeLoad {
        details: games::details(client, &universes).await?,
        votes: games::votes(client, &universes).await.unwrap_or_default(),
        art: thumbnails::game_art(client, &universes).await.unwrap_or_default(),
    })
}

/// Apply a decoded image to one row of a tile model.
///
/// Always called from a deferred context (see `images::Images::load`), never
/// synchronously from a repeater delegate — that path panics with a RefCell
/// double borrow.
fn set_tile_thumb(model: &slint::ModelRc<GameTile>, index: usize, img: slint::Image) {
    use slint::Model;
    if let Some(mut row) = model.row_data(index) {
        row.thumb = img;
        model.set_row_data(index, row);
    }
}

/// Roblox uses this for result continuity across search pages. It only needs
/// to be stable within a session and distinct between them.
fn new_session_id() -> String {
    format!(
        "rojoin-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    )
}
