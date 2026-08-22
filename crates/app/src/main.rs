#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};

use rojoin_launcher::JoinRequest;
use rojoin_roblox::{auth, discovery, friends, games, groups, search, thumbnails, users, Client};
use rojoin_store::Config;
use slint::ComponentHandle;

slint::include_modules!();

mod adapters;
mod bridge;
#[cfg(debug_assertions)]
mod demo;
mod images;
mod linkhandler;
mod notify;
mod secrets;
mod fflags;
mod shortcut;
mod updater;

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
    ("Search", "⌕"),
    ("Friends", "◑"),
    ("Library", "▤"),
    ("Avatar", "☺"),
    ("Macros", "⌨"),
];

/// Shared app state. Guarded by mutexes because background tasks touch it too.
pub(crate) struct App {
    client: Client,
    pub(crate) config: Mutex<Config>,
    /// The universe currently open in the detail view, for follow-up fetches.
    current_universe: Mutex<i64>,
    current_place: Mutex<i64>,
    /// Live quick-login attempt, if any.
    signin: Mutex<Option<auth::LoginCode>>,
    /// Section to return to when a pushed detail view is dismissed.
    return_section: Mutex<i32>,
    search_session: Mutex<String>,

    /// Signed-in user id, needed for friends and group calls.
    me: Mutex<i64>,
    /// The friends roster, kept so filtering and pin toggles can re-render
    /// without another round trip to Roblox.
    roster: Mutex<Vec<ad::FriendInput>>,
    offline_collapsed: Mutex<bool>,
    /// Asset ids currently worn. Local source of truth for the avatar editor,
    /// because Roblox only accepts the complete list on every change.
    worn: Mutex<Vec<i64>>,
    /// Thumbnail loader. Settings renders account avatars from deep inside the
    /// hotkey thread and the toggle macros, where threading a `&Images` through
    /// every call site does not survive the borrow checker.
    imgs: Mutex<Option<Images>>,
    /// The macro engine, absent when input permission is missing.
    engine: Mutex<Option<Arc<rojoin_macro::Engine>>>,
    macros: Mutex<Vec<rojoin_macro::Macro>>,
    /// Held so the launch path can mint a Windows auth ticket without every
    /// caller having to thread the bridge through.
    bridge: Mutex<Option<Arc<Bridge>>>,
    /// Guards against spawning a second presence watcher on account switch.
    watching: std::sync::atomic::AtomicBool,
    /// True while a friend-request accept/decline is in flight. Roblox
    /// rate-limits these hard, so they are serialised.
    request_busy: std::sync::atomic::AtomicBool,
    /// Held so the listener threads live as long as the app does.
    hotkeys: Mutex<Option<rojoin_macro::hotkeys::Listener>>,
    /// Usernames and head-shot URLs resolved once and kept forever.
    ///
    /// This is the single most important cache in the app. Resolving names on
    /// demand is what gets the *account* rate-limited — not just RoJoin — and
    /// it shows up as friends rendering as "User 12345" or vanishing entirely.
    names: Mutex<rojoin_store::NameCache>,
    /// Friend requests accepted or declined this session. Roblox keeps
    /// returning them for a while, so they are filtered out of every refetch.
    handled_requests: Mutex<std::collections::HashSet<String>>,
    /// Macro currently open in the step editor.
    editing: Mutex<Option<String>>,
    /// True while the editor is waiting for a key to bind. The hotkey listener
    /// consumes the next press instead of firing macros.
    capturing: std::sync::atomic::AtomicBool,
    /// Same, for the panic key in Settings.
    binding_panic: std::sync::atomic::AtomicBool,
    /// The FastFlag catalogue, once fetched. ~22,500 entries, so it is fetched
    /// at most once per session and filtered in memory.
    flag_catalog: Mutex<Vec<fflags::Flag>>,
    /// The worn list we last wrote successfully, and when.
    ///
    /// Roblox's avatar read-back is eventually consistent: fetching straight
    /// after a change can still report the previous set. Without this, any
    /// refresh landing in that window resurrects an item the user just took
    /// off — and the next toggle then sends the stale list back, genuinely
    /// re-equipping it server-side.
    worn_write: Mutex<Option<(std::time::Instant, Vec<i64>)>>,
    /// An avatar write is in flight. Clicking items faster than Roblox answers
    /// used to start a parallel write *and* a parallel retry chain for each
    /// one, which is how three follow clicks earned a 429.
    avatar_busy: std::sync::atomic::AtomicBool,
    /// Guards the About me save against a double click.
    bio_busy: std::sync::atomic::AtomicBool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read the config before anything else: it decides how loudly to log, how
    // patient the HTTP client is, and how much of the thumbnail cache to keep.
    let startup = Config::load();

    let level = if startup.settings.verbose_logging {
        "rojoin=debug,rojoin_v4=debug"
    } else {
        "rojoin=info,rojoin_v4=info"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| level.into());

    // Verbose mode also writes to a file, because the interesting failures
    // (a gated write, a throttle) happen while nobody is watching a terminal.
    if startup.settings.verbose_logging {
        match std::fs::File::create(log_path()) {
            Ok(file) => tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .init(),
            Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
        }
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    images::set_capacity(startup.image_cache_size() as usize);
    updater::clean_previous_build();

    let ui = MainWindow::new()?;
    let bridge = Arc::new(Bridge::new(&ui)?);
    let client = Client::with_timeout(startup.request_timeout_secs())?;
    let imgs = Images::new(client.clone(), ui.as_weak(), bridge.runtime().clone());

    let app = Arc::new(App {
        client: client.clone(),
        config: Mutex::new(startup),
        current_universe: Mutex::new(0),
        current_place: Mutex::new(0),
        signin: Mutex::new(None),
        return_section: Mutex::new(0),
        search_session: Mutex::new(new_session_id()),
        me: Mutex::new(0),
        roster: Mutex::new(Vec::new()),
        offline_collapsed: Mutex::new(false),
        worn: Mutex::new(Vec::new()),
        imgs: Mutex::new(None),
        engine: Mutex::new(None),
        macros: Mutex::new(Vec::new()),
        bridge: Mutex::new(None),
        watching: std::sync::atomic::AtomicBool::new(false),
        request_busy: std::sync::atomic::AtomicBool::new(false),
        hotkeys: Mutex::new(None),
        names: Mutex::new(rojoin_store::NameCache::load()),
        handled_requests: Mutex::new(
            Config::load()
                .data()
                .map(|d| d.handled_requests.iter().cloned().collect())
                .unwrap_or_default(),
        ),
        editing: Mutex::new(None),
        capturing: std::sync::atomic::AtomicBool::new(false),
        binding_panic: std::sync::atomic::AtomicBool::new(false),
        flag_catalog: Mutex::new(Vec::new()),
        worn_write: Mutex::new(None),
        avatar_busy: std::sync::atomic::AtomicBool::new(false),
        bio_busy: std::sync::atomic::AtomicBool::new(false),
    });
    *app.bridge.lock().unwrap() = Some(bridge.clone());
    *app.imgs.lock().unwrap() = Some(imgs.clone());

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
    wire_nav(&ui, &app, &bridge);
    wire_search(&ui, &app, &bridge, &imgs);
    wire_game(&ui, &app, &bridge, &imgs);
    wire_launch(&ui, &app, &bridge);
    wire_friends(&ui, &app, &bridge, &imgs);
    wire_profile(&ui, &app, &bridge, &imgs);
    wire_avatar(&ui, &app, &bridge, &imgs);
    wire_macros(&ui, &app);
    wire_settings(&ui, &app, &bridge, &imgs);
    wire_more_settings(&ui, &app);
    wire_client_settings(&ui, &app, &bridge);
    wire_home(&ui, &app, &bridge, &imgs);

    #[cfg(debug_assertions)]
    let demo_mode = demo::enabled();
    #[cfg(not(debug_assertions))]
    let demo_mode = false;

    if demo_mode {
        #[cfg(debug_assertions)]
        demo::seed(&ui);
    } else {
        restore_session(&ui, &app, &bridge, &imgs);
        maybe_auto_update(&ui, &app, &bridge);
    }

    if let Some((place_id, instance)) = std::env::args()
        .skip(1)
        .find_map(|a| linkhandler::parse_uri(&a))
    {
        tracing::info!(place_id, "launching from a deep link");
        let mut req = JoinRequest::place(place_id);
        if let Some(job) = instance {
            req = req.server(job);
        }
        launch(&ui, &app, req);
    }

    // Restore the saved zoom. It has to wait for the event loop: before the
    // window is shown it has no size, and the scale is applied relative to it.
    // Without this the setting silently reset to 1.0 on every launch while
    // Settings still displayed the saved value.
    {
        let weak = ui.as_weak();
        let scale = app.config.lock().unwrap().ui_scale();
        if (scale - 1.0).abs() > f32::EPSILON {
            let timer = Box::leak(Box::new(slint::Timer::default()));
            timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(120),
                move || {
                    if let Some(ui) = weak.upgrade() {
                        apply_ui_scale(&ui, scale);
                    }
                },
            );
        }
    }

    ui.run()?;
    Ok(())
}

/// `unwrap_or_default()`, but it says what broke first.
///
/// Swallowing these is how a dead endpoint hid in plain sight: game passes
/// 404'd for every game on the platform and the detail page simply rendered an
/// empty section for as long as nobody thought to check. Defaulting is still
/// the right behaviour — one missing strip should not take down a whole page —
/// but it has to leave a trace in the log.
fn or_default<T: Default>(what: &str, result: rojoin_roblox::Result<T>) -> T {
    match result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, what, "could not be loaded; showing nothing");
            T::default()
        }
    }
}

/// Try the stored cookie.
///
/// Only a 401 means the login is actually gone. Anything else — no network yet,
/// a throttle, a captcha challenge — says nothing about whether the cookie is
/// still good, and treating those as a sign-out is what made the app look like
/// it kept forgetting the login: the cookie was still in the keyring, but the
/// user was staring at a sign-in screen. Transient failures are retried, and
/// the stored session is left exactly where it is.
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

        // Patient on purpose. A desktop launched at login is routinely up
        // before its network is, and the first attempt fails for reasons that
        // have nothing to do with the account.
        const WAITS: [u64; 4] = [0, 2, 5, 15];
        let mut last: Option<rojoin_roblox::Error> = None;

        for (attempt, wait) in WAITS.iter().enumerate() {
            if *wait > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(*wait)).await;
            }

            match users::authenticated(&client).await {
                Ok(me) => {
                    let app = app.clone();
                    let bridge2 = bridge2.clone();
                    let imgs = imgs.clone();
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        tracing::info!(user = %me.name, "restored session");
                        ui.set_signin_error(Default::default());
                        enter_app(&ui, &app, &bridge2, &imgs, &me.display_name, me.id);
                    });
                    return;
                }

                // The one failure that really is a signed-out account.
                Err(rojoin_roblox::Error::Expired) => {
                    tracing::info!("stored session is no longer valid");
                    let _ = weak.upgrade_in_event_loop(|ui| {
                        ui.set_signed_in(false);
                        ui.set_signin_error(
                            "Roblox ended this session, so signing in again is needed.".into(),
                        );
                    });
                    return;
                }

                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        attempt = attempt + 1,
                        "could not reach Roblox to restore the session"
                    );
                    last = Some(e);
                }
            }
        }

        // Out of retries. The cookie is untouched and still stored, so say so —
        // this is a connection problem, not a lost login.
        let detail = last.map(|e| e.to_string()).unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_signed_in(false);
            ui.set_signin_error(
                format!(
                    "Could not reach Roblox ({detail}). Your saved login is still \
                     here — restart RoJoin once you are back online."
                )
                .into(),
            );
        });
    });
}

/// Wipe everything that belongs to whoever was signed in a moment ago.
///
/// The reloads below are asynchronous, so without this the previous account's
/// library, friends and profile stay on screen until each request lands — long
/// enough to look like the switch did not happen.
fn clear_account_view(ui: &MainWindow) {
    let empty = || ad::model(Vec::<GameTile>::new());
    ui.set_recent(empty());
    ui.set_favorites(empty());
    ui.set_pinned(empty());
    ui.set_play_games(empty());
    ui.set_related(empty());
    ui.set_profile_favorites(empty());
    ui.set_group_games(empty());

    ui.set_friend_rows(ad::model(Vec::new()));
    ui.set_requests_list(ad::model(Vec::new()));
    ui.set_profile_groups(ad::model(Vec::new()));
    ui.set_profile_badges(ad::model(Vec::new()));
    ui.set_av_items(ad::model(Vec::new()));
    ui.set_av_outfits(ad::model(Vec::new()));
    ui.set_av_worn(ad::model(Vec::new()));
    ui.set_most_played(ad::model(Vec::new()));
    ui.set_servers(ad::model(Vec::new()));
    ui.set_sub_places(ad::model(Vec::new()));
    ui.set_passes(ad::model(Vec::new()));
    ui.set_badges(ad::model(Vec::new()));
    ui.set_play_players(ad::model(Vec::new()));
    ui.set_play_groups(ad::model(Vec::new()));

    ui.set_total_playtime("0m".into());
    ui.set_games_played(0);
    ui.set_total_launches(0);
    ui.set_friends_online(0);
    ui.set_friend_requests(0);
    ui.set_friends_in_game(0);
    ui.set_av_worn_count(0);
    ui.set_request_status("".into());
    ui.set_play_last_query("".into());

    ui.set_account_avatar(slint::Image::default());
    ui.set_launch_error("".into());
    ui.set_account_mismatch(false);

    // Any open profile or game page belonged to the old session.
    ui.set_view_kind(0);
    ui.set_can_back(false);
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
    clear_account_view(ui);

    // Caches that describe the *previous* account. The roster in particular
    // would otherwise be re-rendered under the new account by any filter click
    // that lands before the friends fetch returns.
    app.roster.lock().unwrap().clear();
    app.worn.lock().unwrap().clear();
    *app.current_universe.lock().unwrap() = 0;
    *app.current_place.lock().unwrap() = 0;
    *app.search_session.lock().unwrap() = new_session_id();
    *app.handled_requests.lock().unwrap() = app
        .config
        .lock()
        .unwrap()
        .data()
        .map(|d| d.handled_requests.iter().cloned().collect())
        .unwrap_or_default();

    ui.set_signed_in(true);
    ui.set_session_expired(false);
    ui.set_account_name(display_name.into());
    ui.set_accounts_count(app.config.lock().unwrap().accounts.len() as i32);
    *app.me.lock().unwrap() = user_id;

    let imgs2 = imgs.clone();
    let client = app.client.clone();
    bridge.spawn(async move {
        if let Ok(map) = thumbnails::headshots(&client, &[user_id]).await {
            if let Some(url) = map.get(&user_id).cloned() {
                imgs2.load(&url, move |ui, img| ui.set_account_avatar(img));
            }
        }
    });

    ui.set_section(
        app.config
            .lock()
            .unwrap()
            .settings
            .startup_section
            .clamp(0, NAV.len() as i32 - 1),
    );

    check_client_account(ui, app, bridge);

    apply_home_sections(ui, app);
    load_home(ui, app, bridge, imgs);
    load_pinned(ui, app, bridge, imgs);
    render_library(ui, app);
    render_settings(ui, app);

    stagger(ui, app, bridge, imgs, 300, load_favorites);
    stagger(ui, app, bridge, imgs, 600, load_friends);
    stagger(ui, app, bridge, imgs, 2_000, load_avatar);

    if !app.watching.swap(true, Ordering::SeqCst) {
        let watcher = notify::Watcher::new(app.client.clone(), app.clone());
        bridge.spawn(async move { watcher.run().await });
    }
}

/// Run a loader after `delay_ms`, dropping it if the account changed meanwhile.
fn stagger(
    ui: &MainWindow,
    app: &Arc<App>,
    bridge: &Arc<Bridge>,
    imgs: &Images,
    delay_ms: u64,
    load: fn(&MainWindow, &Arc<App>, &Arc<Bridge>, &Images),
) {
    let weak = ui.as_weak();
    let app = app.clone();
    let bridge = bridge.clone();
    let imgs = imgs.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(delay_ms),
        move || {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                load(&ui, &app, &bridge, &imgs);
            }
        },
    );
    STAGGER_TIMERS.with(|t| t.borrow_mut().push(timer));
}

thread_local! {
    static STAGGER_TIMERS: std::cell::RefCell<Vec<slint::Timer>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Warn when the Roblox client is signed into a different account.
///
/// Launching rewrites the client's session, so silently swapping it out from
/// under someone is the kind of surprise worth pre-empting.
fn check_client_account(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>) {
    ui.set_account_mismatch(false);

    let Some(mine) = app
        .config
        .lock()
        .unwrap()
        .active_account
        .clone()
        .and_then(|id| secrets::load(&id))
    else {
        return;
    };

    let Some(theirs) = rojoin_launcher::sober::read_cookie() else { return };
    if theirs == mine {
        return;
    }

    bridge.call_res(
        move || async move {
            let probe = Client::new()?;
            probe.set_cookie(Some(theirs)).await;
            users::authenticated(&probe).await
        },
        move |ui, result| match result {
            Ok(who) => {
                ui.set_client_account(who.display_name.into());
                ui.set_account_mismatch(true);
            }
            Err(e) => tracing::debug!(error = %e, "could not identify the client's account"),
        },
    );
}

fn wire_friends(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_refresh_friends(move || load_friends(&weak.unwrap(), &app, &bridge2, &imgs2));
    }

    {
        let app = app.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_friend_filter_changed(move |_| recompute_friends(&weak.unwrap(), &app, &imgs2));
    }
    {
        let app = app.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_toggle_friend_group(move |label| {
            if label == "OFFLINE" {
                let mut c = app.offline_collapsed.lock().unwrap();
                *c = !*c;
            }
            recompute_friends(&weak.unwrap(), &app, &imgs2);
        });
    }
    {
        let app = app.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_pin_friend(move |id| {
            {
                let mut cfg = app.config.lock().unwrap();
                let data = cfg.data_mut();
                let key = id.to_string();
                if let Some(pos) = data.pinned_friends.iter().position(|p| *p == key) {
                    data.pinned_friends.remove(pos);
                } else {
                    data.pinned_friends.push(key);
                }
                let _ = cfg.save();
            }
            recompute_friends(&weak.unwrap(), &app, &imgs2);
        });
    }
    {
        let app = app.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_notify_friend(move |id| {
            {
                let mut cfg = app.config.lock().unwrap();
                let key = id.to_string();
                if let Some(pos) = cfg.settings.notify_friends.iter().position(|p| *p == key) {
                    cfg.settings.notify_friends.remove(pos);
                } else {
                    cfg.settings.notify_friends.push(key);
                }
                let _ = cfg.save();
            }
            recompute_friends(&weak.unwrap(), &app, &imgs2);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_join_friend(move |id| {
            let ui = weak.unwrap();
            let Ok(fid) = id.parse::<i64>() else { return };

            let target = app
                .roster
                .lock()
                .unwrap()
                .iter()
                .find(|f| f.id == fid)
                .and_then(|f| f.place_id.map(|p| (p, f.game_id.clone())));

            let Some((place_id, job)) = target else {
                tracing::warn!(%id, "friend is not in a joinable game");
                return;
            };

            let mut req = JoinRequest::place(place_id);
            if let Some(job) = job {
                req = req.server(job);
            }
            launch(&ui, &app, req);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_accept_request(move |id| {
            respond_to_request(&weak.unwrap(), &app, &bridge2, &imgs2, id.as_str(), true)
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_decline_request(move |id| {
            respond_to_request(&weak.unwrap(), &app, &bridge2, &imgs2, id.as_str(), false)
        });
    }

}

fn load_friends(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let me = *app.me.lock().unwrap();
    if me == 0 {
        return;
    }

    ui.set_friends_loading(true);
    let client = app.client.clone();
    let app2 = app.clone();
    let imgs2 = imgs.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    let cached = app.names.lock().unwrap().users.clone();

    bridge.call_res(
        move || async move { fetch_friends(&client, me, cached).await },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_friends_loading(false);

            let load = match result {
                Ok(load) => load,
                Err(e) => return bridge::report(&ui, e),
            };

            let requests: Vec<DetailItem> = load
                .requests
                .iter()
                .map(|u| DetailItem {
                    id: u.id.to_string().into(),
                    name: u.display_name.clone().into(),
                    subtitle: u.name.clone().into(),
                    thumb: slint::Image::default(),
                    kind: 8,
                })
                .collect();

            let handled = app2.handled_requests.lock().unwrap().clone();
            let requests: Vec<DetailItem> = requests
                .into_iter()
                .filter(|r| !handled.contains(&r.id.to_string()))
                .collect();
            let count = requests.len() as i32;
            ui.set_requests_list(ad::model(requests));
            ui.set_friend_requests(count);

            for (i, u) in load.requests.iter().enumerate() {
                if let Some(url) = load.avatars.get(&u.id).cloned() {
                    let id = u.id.to_string();
                    imgs2.load(&url, move |ui, img| {
                        set_item_thumb(&ui.get_requests_list(), i, &id, img)
                    });
                }
            }

            if !load.resolved.is_empty() {
                let mut names = app2.names.lock().unwrap();
                for (id, entry) in load.resolved {
                    names.users.insert(id.to_string(), entry);
                }
                if let Err(e) = names.save() {
                    tracing::warn!(error = %e, "could not persist the name cache");
                }
            }

            *app2.roster.lock().unwrap() = load.friends;
            recompute_friends(&ui, &app2, &imgs2);
        },
    );
}

/// Rebuild the friends model from the cached roster. Pure UI-thread work.
fn recompute_friends(ui: &MainWindow, app: &Arc<App>, imgs: &Images) {
    let (pinned, notify) = {
        let cfg = app.config.lock().unwrap();
        (
            cfg.data()
                .map(|d| d.pinned_friends.iter().cloned().collect())
                .unwrap_or_default(),
            cfg.settings.notify_friends.iter().cloned().collect(),
        )
    };

    let roster = app.roster.lock().unwrap();
    let collapsed = *app.offline_collapsed.lock().unwrap();
    let view = ad::friend_rows(
        &roster,
        &pinned,
        &notify,
        ui.get_friend_filter().as_str(),
        collapsed,
    );

    ui.set_friends_in_game(view.in_game);
    ui.set_friends_online(view.online);

    let urls: Vec<(usize, String)> = view
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.is_header)
        .filter_map(|(i, r)| {
            let id = r.id.parse::<i64>().ok()?;
            let f = roster.iter().find(|f| f.id == id)?;
            (!f.avatar_url.is_empty()).then(|| (i, f.avatar_url.clone()))
        })
        .collect();
    drop(roster);

    ui.set_friend_rows(ad::model(view.rows));

    for (i, url) in urls {
        imgs.load(&url, move |ui, img| {
            set_friend_avatar(&ui.get_friend_rows(), i, img)
        });
    }
}

fn respond_to_request(
    ui: &MainWindow,
    app: &Arc<App>,
    bridge: &Arc<Bridge>,
    imgs: &Images,
    id: &str,
    accept: bool,
) {
    let Ok(user_id) = id.parse::<i64>() else {
        tracing::error!(%id, "friend request has an unparseable user id");
        return;
    };

    if app.request_busy.swap(true, Ordering::SeqCst) {
        ui.set_request_status("One at a time — still working on the last one.".into());
        return;
    }

    tracing::info!(user_id, accept, "responding to friend request");
    ui.set_request_status(if accept { "Accepting…".into() } else { "Declining…".into() });

    let client = app.client.clone();
    let app2 = app.clone();
    let bridge2 = bridge.clone();
    let imgs2 = imgs.clone();

    bridge.call_res(
        move || async move {
            let r = if accept {
                friends::accept(&client, user_id).await
            } else {
                friends::decline(&client, user_id).await
            };
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            r
        },
        move |ui, result| {
            app2.request_busy.store(false, Ordering::SeqCst);

            match result {
                Ok(()) => {
                    tracing::info!(user_id, "friend request handled");
                    ui.set_request_status("".into());

                    {
                        let key = user_id.to_string();
                        app2.handled_requests.lock().unwrap().insert(key.clone());
                        let mut cfg = app2.config.lock().unwrap();
                        if cfg.remember_handled_request(key) {
                            let _ = cfg.save();
                        }
                    }

                    drop_request_row(&ui, user_id);
                    load_friends(&ui, &app2, &bridge2, &imgs2);
                }
                Err(rojoin_roblox::Error::RateLimited) => {
                    ui.set_request_status(
                        "Roblox is rate-limiting friend requests. Wait a moment and try again."
                            .into(),
                    );
                }
                Err(e) => {
                    ui.set_request_status(format!("Could not do that: {e}").into());
                    bridge::report(&ui, e);
                }
            }
        },
    );
}

/// Remove a handled request from the visible list.
fn drop_request_row(ui: &MainWindow, user_id: i64) {
    use slint::Model;
    let model = ui.get_requests_list();
    let key = user_id.to_string();
    let kept: Vec<DetailItem> = model.iter().filter(|r| r.id != key).collect();
    let remaining = kept.len() as i32;
    ui.set_requests_list(ad::model(kept));
    ui.set_friend_requests(remaining);
}

struct FriendsLoad {
    friends: Vec<ad::FriendInput>,
    requests: Vec<rojoin_roblox::models::User>,
    avatars: std::collections::HashMap<i64, String>,
    /// Users resolved this round, to be folded into the persistent cache.
    resolved: Vec<(i64, rojoin_store::CachedUser)>,
}

async fn fetch_friends(
    client: &Client,
    me: i64,
    cached: std::collections::HashMap<String, rojoin_store::CachedUser>,
) -> rojoin_roblox::Result<FriendsLoad> {
    let list = friends::friend_ids(client, me).await?;
    if !list.complete {
        tracing::warn!(count = list.ids.len(), "friends list may be incomplete");
    }

    let unknown: Vec<i64> = list
        .ids
        .iter()
        .copied()
        .filter(|id| !cached.contains_key(&id.to_string()))
        .collect();

    let to_resolve: Vec<i64> = unknown.iter().copied().take(users::BATCH).collect();

    let fetched = if to_resolve.is_empty() {
        Vec::new()
    } else {
        tracing::info!(
            resolving = to_resolve.len(),
            outstanding = unknown.len(),
            "resolving unseen users"
        );
        or_default("usernames", users::batch(client, &to_resolve).await)
    };

    let presence = or_default("friend presence", friends::presence(client, &list.ids).await);
    let requests = or_default("friend requests", friends::requests(client, 25).await);

    let mut avatar_ids: Vec<i64> = fetched.iter().map(|u| u.id).collect();
    avatar_ids.extend(requests.iter().map(|u| u.id));
    let mut avatars = if avatar_ids.is_empty() {
        Default::default()
    } else {
        or_default("friend avatars", thumbnails::headshots(client, &avatar_ids).await)
    };
    for (id, entry) in &cached {
        if let Ok(id) = id.parse::<i64>() {
            if !entry.avatar_url.is_empty() {
                avatars.entry(id).or_insert_with(|| entry.avatar_url.clone());
            }
        }
    }

    let now = chrono::Utc::now().timestamp();
    let resolved: Vec<(i64, rojoin_store::CachedUser)> = fetched
        .iter()
        .map(|u| {
            (
                u.id,
                rojoin_store::CachedUser {
                    username: u.name.clone(),
                    display_name: u.display_name.clone(),
                    avatar_url: avatars.get(&u.id).cloned().unwrap_or_default(),
                    verified: u.has_verified_badge,
                    ts: now,
                },
            )
        })
        .collect();

    let users: Vec<rojoin_roblox::models::User> = list
        .ids
        .iter()
        .map(|id| {
            if let Some(u) = fetched.iter().find(|u| u.id == *id) {
                return u.clone();
            }
            match cached.get(&id.to_string()) {
                Some(c) => rojoin_roblox::models::User {
                    id: *id,
                    name: c.username.clone(),
                    display_name: c.display_name.clone(),
                    has_verified_badge: c.verified,
                    ..Default::default()
                },
                None => rojoin_roblox::models::User {
                    id: *id,
                    name: format!("User {id}"),
                    display_name: format!("User {id}"),
                    ..Default::default()
                },
            }
        })
        .collect();

    let friends = users
        .iter()
        .map(|u| {
            let p = presence.iter().find(|p| p.user_id == u.id);
            let kind = p.map(|p| p.kind).unwrap_or(friends::PresenceKind::Offline);
            ad::FriendInput {
                id: u.id,
                name: u.display_name.clone(),
                username: u.name.clone(),
                presence: match kind {
                    friends::PresenceKind::Online => 1,
                    friends::PresenceKind::InGame => 2,
                    friends::PresenceKind::InStudio => 3,
                    _ => 0,
                },
                location: p.map(|p| p.location.clone()).unwrap_or_default(),
                last_online: p.and_then(|p| p.last_online.clone()).unwrap_or_default(),
                place_id: p.and_then(|p| p.place_id.or(p.root_place_id)),
                game_id: p.and_then(|p| p.game_id.clone()),
                avatar_url: avatars.get(&u.id).cloned().unwrap_or_default(),
                joinable: kind.is_joinable() && p.and_then(|p| p.place_id).is_some(),
            }
        })
        .collect();

    Ok(FriendsLoad { friends, requests, avatars, resolved })
}

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
    {
        let weak = ui.as_weak();
        ui.on_dismiss_mismatch(move || weak.unwrap().set_account_mismatch(false));
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

fn wire_nav(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>) {
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
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_toggle_notify(move || {
            let ui = weak.unwrap();
            let id = ui.get_game().universe_id.to_string();
            if id.is_empty() {
                return;
            }
            let mut g = ui.get_game();
            let on = !g.notify;
            g.notify = on;
            ui.set_game(g);

            let mut cfg = app.config.lock().unwrap();
            if on {
                if !cfg.settings.notify_games.contains(&id) {
                    cfg.settings.notify_games.push(id);
                }
            } else {
                cfg.settings.notify_games.retain(|g| *g != id);
            }
            let _ = cfg.save();
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_copy_code(move || {
            let ui = weak.unwrap();
            let code = ui.get_signin_code().to_string();
            if code.is_empty() {
                return;
            }
            copy_with_toast(&ui, &code, "code");
            ui.set_code_copied(true);
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_copy_link(move || {
            let ui = weak.unwrap();
            let url = current_url(&ui);
            if !url.is_empty() {
                copy_with_toast(&ui, &url, "link");
            }
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_make_shortcut(move || {
            let ui = weak.unwrap();
            let game = ui.get_game();
            let Ok(place_id) = game.root_place_id.parse::<i64>() else { return };
            let Ok(universe_id) = game.universe_id.parse::<i64>() else { return };
            let name = game.name.to_string();

            ui.set_launch_error("".into());
            let client = app.client.clone();

            bridge2.call_res(
                move || async move {
                    let icons = thumbnails::game_icons(&client, &[universe_id])
                        .await
                        .unwrap_or_default();

                    let icon_bytes = match icons.get(&universe_id) {
                        Some(url) => client.fetch_bytes(url).await.ok(),
                        None => None,
                    };
                    Ok((name, icon_bytes))
                },
                move |ui, result: rojoin_roblox::Result<(String, Option<Vec<u8>>)>| {
                    let Ok((name, icon_bytes)) = result else {
                        ui.set_launch_error("Could not fetch the game icon.".into());
                        return;
                    };

                    let icon = icon_bytes
                        .as_deref()
                        .and_then(|b| shortcut::save_icon(place_id, b));

                    match shortcut::create(place_id, &name, icon.as_deref()) {
                        Ok(path) => {
                            tracing::info!(path = %path.display(), "created desktop shortcut");
                            ui.set_launch_error(
                                format!("Shortcut saved to {}", path.display()).into(),
                            );
                        }
                        Err(e) => ui.set_launch_error(e.into()),
                    }
                },
            );
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_inspect_badge(move |id| {
            use slint::Model;
            let ui = weak.unwrap();
            let Some(b) = ui.get_badges().iter().find(|b| b.id == id) else { return };
            ui.set_badge_title(b.name.clone());
            ui.set_badge_text(if b.subtitle.is_empty() {
                "No description.".into()
            } else {
                b.subtitle.clone()
            });
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_copy_id(move || {
            let ui = weak.unwrap();
            let id = match ui.get_view_kind() {
                1 => ui.get_game().root_place_id.to_string(),
                2 => ui.get_profile().id.to_string(),
                3 => ui.get_group().id.to_string(),
                _ => String::new(),
            };
            if !id.is_empty() {
                copy_with_toast(&ui, &id, "ID");
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_open_browser(move || {
            let ui = weak.unwrap();
            let url = current_url(&ui);
            if !url.is_empty() {
                let _ = webbrowser::open(&url);
            }
        });
    }
}

/// The roblox.com URL for whatever is on screen.
fn current_url(ui: &MainWindow) -> String {
    match ui.get_view_kind() {
        1 => {
            let id = ui.get_game().root_place_id.to_string();
            (!id.is_empty())
                .then(|| format!("https://www.roblox.com/games/{id}"))
                .unwrap_or_default()
        }
        2 => {
            let id = ui.get_profile().id.to_string();
            (!id.is_empty())
                .then(|| format!("https://www.roblox.com/users/{id}/profile"))
                .unwrap_or_default()
        }
        3 => {
            let id = ui.get_group().id.to_string();
            (!id.is_empty())
                .then(|| format!("https://www.roblox.com/groups/{id}"))
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// Copy, and say so. Every call site wants the confirmation, so it lives here
/// rather than being repeated (and eventually forgotten) at each one.
fn copy_with_toast(ui: &MainWindow, text: &str, what: &str) {
    copy_to_clipboard(text);
    toast(ui, &format!("Copied {what}"));
}

fn toast(ui: &MainWindow, message: &str) {
    ui.set_toast_text(message.into());
    ui.set_toast_nonce(ui.get_toast_nonce().wrapping_add(1));
}

/// Where verbose logs are written. Beside the config, so the existing
/// "Data folder" button in Settings already reveals it.
/// Reveal the log file, or the folder holding it if it has not been written
/// yet — opening a missing path just fails silently otherwise.
fn open_log_file() {
    let path = log_path();
    let target = if path.is_file() { path } else { rojoin_store::config_dir() };
    let _ = std::process::Command::new(if cfg!(windows) { "explorer" } else { "xdg-open" })
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Fetch and stage a newer release in the background, if one exists.
///
/// The running binary is never swapped underneath a live process — the new
/// build only takes over on the next launch — so this is safe to do silently.
/// It stays quiet unless it actually did something.
fn maybe_auto_update(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>) {
    if !app.config.lock().unwrap().settings.auto_update {
        return;
    }

    let weak = ui.as_weak();
    bridge.spawn(async move {
        let updater::Status::Available { version, url } = updater::check().await else {
            return;
        };

        match updater::install(&url).await {
            Ok(_) => {
                tracing::info!(%version, "update staged");
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    toast(&ui, &format!("Updated to {version} — restart to apply"));
                    ui.set_update_status(
                        format!("Version {version} installed. Restart to use it.").into(),
                    );
                });
            }
            Err(e) => tracing::warn!(error = %e, "auto-update could not finish"),
        }
    });
}

/// Apply an interface zoom level.
///
/// `ScaleFactorChanged` on its own only changes how logical pixels map to
/// physical ones — the OS window keeps its old size, so the content either
/// overflows past the edge or leaves a dead margin inside it. Slint has to be
/// told the new *logical* size as well, which is the same physical window
/// measured in the new units. The window itself deliberately does not move or
/// resize: this is a zoom, not a resize.
fn apply_ui_scale(ui: &MainWindow, scale: f32) {
    let window = ui.window();
    let physical = window.size();
    if physical.width == 0 || physical.height == 0 {
        return;
    }

    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    window.dispatch_event(slint::platform::WindowEvent::Resized {
        size: slint::LogicalSize::new(
            physical.width as f32 / scale,
            physical.height as f32 / scale,
        ),
    });
}

/// Mirror Sober's own settings into the CLIENT card.
///
/// Read fresh each time rather than cached: Sober rewrites this file when it
/// exits, so anything remembered from earlier in the session may be stale.
fn render_client_settings(ui: &MainWindow) {
    use rojoin_launcher::sober;

    let Some(config) = sober::read_config() else {
        ui.set_has_client(false);
        return;
    };
    ui.set_has_client(true);

    let flag = |key: &str| config.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
    ui.set_client_opengl(flag("use_opengl"));
    ui.set_client_hidpi(flag("enable_hidpi"));
    ui.set_client_gamemode(flag("enable_gamemode"));
    ui.set_client_close_on_leave(flag("close_on_leave"));
    ui.set_client_discord(flag("discord_rpc_enabled"));
    ui.set_client_region(flag("server_location_indicator_enabled"));

    ui.set_client_graphics(
        match config.get("graphics_optimization_mode").and_then(|v| v.as_str()) {
            Some("quality") => 0,
            Some("performance") => 2,
            _ => 1,
        },
    );

}

/// Rebuild the flag list for whatever is typed in the search box.
///
/// Flags the user has actually set always show, regardless of the filter, so a
/// stray search cannot hide something that is currently in effect.
fn render_flag_rows(ui: &MainWindow, app: &Arc<App>) {
    // The account's own flags are the source of truth; Sober's config is
    // written from them before a launch.
    let set = app.config.lock().unwrap().fflags();
    let catalog = app.flag_catalog.lock().unwrap();
    let needle = ui.get_flag_filter().to_string().to_lowercase();

    ui.set_flag_total(catalog.len() as i32);

    let describe = |name: &str| -> (String, String) {
        let from_catalog = catalog.iter().find(|f| f.name == name);
        let note = from_catalog
            .and_then(|f| f.note)
            .or_else(|| {
                fflags::documented()
                    .into_iter()
                    .find(|d| d.name == name)
                    .and_then(|d| d.note)
            })
            .unwrap_or_default()
            .to_string();
        (from_catalog.map(|f| f.default.clone()).unwrap_or_default(), note)
    };

    let mut rows: Vec<FlagRow> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Whatever is actually set comes first and always shows, filter or not:
    // hiding a flag that is in effect would be actively misleading.
    let mut active: Vec<(&String, &String)> = set.iter().collect();
    active.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in active {
        let (default, note) = describe(name);
        seen.insert(name.clone());
        rows.push(FlagRow {
            name: name.clone().into(),
            default_value: default.into(),
            note: note.into(),
            set_value: value.clone().into(),
        });
    }

    // Then the documented ones. These come from our own list rather than the
    // download, because most override flags are never published and would
    // otherwise be missing from the very screen meant to explain them.
    let matches = |name: &str| needle.len() < 2 || name.to_lowercase().contains(&needle);
    for flag in fflags::documented() {
        if seen.contains(&flag.name) || !matches(&flag.name) {
            continue;
        }
        let (default, _) = describe(&flag.name);
        seen.insert(flag.name.clone());
        rows.push(FlagRow {
            name: flag.name.into(),
            default_value: default.into(),
            note: flag.note.unwrap_or_default().into(),
            set_value: Default::default(),
        });
    }

    // Finally the published catalogue, but only once something is typed: 22,500
    // alphabetical internal flags is not a browsing experience.
    if needle.len() >= 2 {
        for flag in catalog
            .iter()
            .filter(|f| !seen.contains(&f.name))
            .filter(|f| f.name.to_lowercase().contains(&needle))
            .take(200)
        {
            rows.push(FlagRow {
                name: flag.name.clone().into(),
                default_value: flag.default.clone().into(),
                note: flag.note.unwrap_or_default().into(),
                set_value: Default::default(),
            });
        }
    }

    ui.set_flag_rows(ad::model(rows));
    ui.set_presets(ad::strings(app.config.lock().unwrap().preset_names()));
}

/// Push the active account's flags into Sober, so a launch uses them.
///
/// Called after every edit and before launching. Sober refuses edits while it
/// is running, so a failure here is reported rather than hidden.
fn sync_fflags_to_client(ui: &MainWindow, app: &Arc<App>) {
    let flags: Vec<(String, String)> = app.config.lock().unwrap().fflags().into_iter().collect();
    match rojoin_launcher::sober::write_fflags(&flags) {
        Ok(()) => ui.set_client_status("".into()),
        Err(e) => ui.set_client_status(format!("{e}").into()),
    }
}

/// Fetch the catalogue, then show it.
fn load_flag_catalog(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>) {
    if !app.flag_catalog.lock().unwrap().is_empty() {
        render_flag_rows(ui, app);
        return;
    }

    ui.set_flags_loading(true);
    let client = app.client.clone();
    let app2 = app.clone();

    bridge.call_res(
        move || async move { Ok::<_, rojoin_roblox::Error>(fflags::catalog(&client).await) },
        move |ui, result| {
            ui.set_flags_loading(false);
            if let Ok(list) = result {
                *app2.flag_catalog.lock().unwrap() = list;
            }
            render_flag_rows(&ui, &app2);
        },
    );
}

/// Everything on the CLIENT card. Each write is refused while Roblox is open,
/// because Sober overwrites its own config on exit.
fn wire_client_settings(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>) {
    use rojoin_launcher::sober;

    fn report(ui: &MainWindow, result: rojoin_launcher::Result<()>) {
        match result {
            Ok(()) => {
                ui.set_client_status("".into());
                render_client_settings(ui);
            }
            Err(e) => ui.set_client_status(format!("{e}").into()),
        }
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_open_client(move || {
            let ui = weak.unwrap();
            tracing::info!("opening the client screen");
            push_view(&ui, &app, 4);
            load_flag_catalog(&ui, &app, &bridge2);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_flag_search(move |_| render_flag_rows(&weak.unwrap(), &app));
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_flag_refresh(move || {
            let ui = weak.unwrap();
            // Drop the in-memory copy so the fetch actually re-runs.
            app.flag_catalog.lock().unwrap().clear();
            load_flag_catalog(&ui, &app, &bridge2);
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_set_client_flag(move |key, value| {
            let ui = weak.unwrap();
            report(&ui, sober::set_config_key(key.as_str(), value.into()));
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_set_client_graphics(move |choice| {
            let ui = weak.unwrap();
            let mode = ["quality", "balanced", "performance"][choice.clamp(0, 2) as usize];
            report(&ui, sober::set_config_key("graphics_optimization_mode", mode.into()));
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_add_fflag(move |name, value| {
            let ui = weak.unwrap();
            // A flag with no value would silently delete it, which is not what
            // pressing "+" means.
            let value = if value.trim().is_empty() { "true" } else { value.as_str() };
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.set_fflag(name.as_str(), value);
                let _ = cfg.save();
            }
            sync_fflags_to_client(&ui, &app);
            render_flag_rows(&ui, &app);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_remove_fflag(move |name| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.set_fflag(name.as_str(), "");
                let _ = cfg.save();
            }
            sync_fflags_to_client(&ui, &app);
            render_flag_rows(&ui, &app);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_save_preset(move |name| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.save_preset(name.as_str());
                let _ = cfg.save();
            }
            render_flag_rows(&ui, &app);
            toast(&ui, "Preset saved");
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_load_preset(move |name| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                let Some(flags) = cfg.settings.fflag_presets.get(name.as_str()).cloned() else {
                    return;
                };
                cfg.set_fflags(flags);
                let _ = cfg.save();
            }
            sync_fflags_to_client(&ui, &app);
            render_flag_rows(&ui, &app);
            toast(&ui, "Preset loaded");
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_delete_preset(move |name| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.delete_preset(name.as_str());
                let _ = cfg.save();
            }
            render_flag_rows(&ui, &app);
        });
    }
}

fn log_path() -> std::path::PathBuf {
    rojoin_store::config_dir().join("rojoin.log")
}

fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    for (bin, args) in [
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
        ("xsel", &["--clipboard", "--input"][..]),
        ("clip.exe", &[][..]),
    ] {
        let Ok(mut child) = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };

        if let Some(stdin) = child.stdin.as_mut() {
            if stdin.write_all(text.as_bytes()).is_ok() {
                drop(child.stdin.take());
                let _ = child.wait();
                tracing::info!("copied to clipboard");
                return;
            }
        }
    }
    tracing::warn!("no clipboard tool available (tried wl-copy, xclip, xsel)");
}

fn wire_profile(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    for hook in ["player", "friend", "creator"] {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        let handler = move |id: slint::SharedString| {
            if let Ok(uid) = id.parse::<i64>() {
                open_profile(&weak.unwrap(), &app, &bridge2, &imgs2, uid);
            }
        };
        match hook {
            "player" => ui.on_open_player(handler),
            "friend" => ui.on_open_friend(handler),
            _ => {}
        }
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_open_creator(move || {
            let ui = weak.unwrap();
            let game = ui.get_game();
            let Ok(id) = game.creator_id.parse::<i64>() else { return };
            if id == 0 {
                return;
            }
            if game.creator_is_group {
                open_group(&ui, &app, &bridge2, &imgs2, id);
            } else {
                open_profile(&ui, &app, &bridge2, &imgs2, id);
            }
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_open_group(move |id| {
            if let Ok(gid) = id.parse::<i64>() {
                open_group(&weak.unwrap(), &app, &bridge2, &imgs2, gid);
            }
        });
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_profile_join(move || {
            let ui = weak.unwrap();
            let Ok(place) = ui.get_profile().place_id.parse::<i64>() else { return };
            launch(&ui, &app, JoinRequest::place(place));
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_profile_accept_request(move || {
            answer_request(&weak.unwrap(), &app, &bridge2, true);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_profile_decline_request(move || {
            answer_request(&weak.unwrap(), &app, &bridge2, false);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_profile_unfriend(move || {
            let ui = weak.unwrap();
            let Ok(uid) = ui.get_profile().id.parse::<i64>() else { return };
            let client = app.client.clone();
            ui.set_profile_busy(true);

            let mut p = ui.get_profile();
            p.is_friend = false;
            ui.set_profile(p);

            bridge2.call_res(
                move || async move { friends::unfriend(&client, uid).await },
                move |ui, result| {
                    ui.set_profile_busy(false);
                    if let Err(e) = result {
                        let mut p = ui.get_profile();
                        p.is_friend = true;
                        ui.set_profile(p);
                        bridge::report(&ui, e);
                    }
                },
            );
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_group_join(move || {
            let ui = weak.unwrap();
            let Ok(gid) = ui.get_group().id.parse::<i64>() else { return };
            let client = app.client.clone();
            ui.set_group_busy(true);
            bridge2.call_res(
                move || async move { groups::join(&client, gid).await },
                move |ui, result| {
                    ui.set_group_busy(false);
                    match result {
                        Ok(()) => {
                            let mut g = ui.get_group();
                            g.is_member = true;
                            g.role = "Member".into();
                            ui.set_group(g);
                        }
                        Err(e) => bridge::report(&ui, e),
                    }
                },
            );
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_group_leave(move || {
            let ui = weak.unwrap();
            let Ok(gid) = ui.get_group().id.parse::<i64>() else { return };
            let me = *app.me.lock().unwrap();
            let client = app.client.clone();
            ui.set_group_busy(true);
            bridge2.call_res(
                move || async move { groups::leave(&client, gid, me).await },
                move |ui, result| {
                    ui.set_group_busy(false);
                    match result {
                        Ok(()) => {
                            let mut g = ui.get_group();
                            g.is_member = false;
                            g.role = "".into();
                            ui.set_group(g);
                        }
                        Err(e) => bridge::report(&ui, e),
                    }
                },
            );
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_open_group_owner(move || {
            let ui = weak.unwrap();
            if let Ok(uid) = ui.get_group().owner_id.parse::<i64>() {
                open_profile(&ui, &app, &bridge2, &imgs2, uid);
            }
        });
    }

}

fn wire_avatar(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    ui.set_av_categories(ad::strings(
        rojoin_roblox::avatar::CATEGORIES
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect(),
    ));

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_av_refresh(move || load_avatar(&weak.unwrap(), &app, &bridge2, &imgs2));
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_av_pick_category(move |_| load_wardrobe(&weak.unwrap(), &app, &bridge2, &imgs2));
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_av_toggle(move |id| {
            let ui = weak.unwrap();
            let Ok(asset_id) = id.parse::<i64>() else { return };

            // One write at a time: this endpoint sends the *whole* worn list,
            // so two in flight race and the loser silently wins.
            if app.avatar_busy.swap(true, Ordering::SeqCst) {
                return;
            }

            let before = app.worn.lock().unwrap().clone();
            let adding = !before.contains(&asset_id);
            let mut after = before.clone();
            if adding {
                after.push(asset_id);
            } else {
                after.retain(|a| *a != asset_id);
            }
            *app.worn.lock().unwrap() = after.clone();

            apply_worn_flags(&ui, &after);
            sync_worn_list(&ui, &after);
            ui.set_av_worn_count(after.len() as i32);
            ui.set_av_busy(true);

            let client = app.client.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs3 = imgs2.clone();
            let send = after.clone();
            let sent = after.clone();

            bridge2.call_res(
                move || async move { rojoin_roblox::avatar::set_wearing(&client, &send).await },
                move |ui, result| {
                    ui.set_av_busy(false);
                    app2.avatar_busy.store(false, Ordering::SeqCst);
                    match result {
                        Ok(()) => {
                            *app2.worn_write.lock().unwrap() =
                                Some((std::time::Instant::now(), sent));
                            refresh_avatar_render(&ui, &app2, &bridge3, &imgs3)
                        }
                        Err(e) => {
                            *app2.worn.lock().unwrap() = before.clone();
                            apply_worn_flags(&ui, &before);
                            sync_worn_list(&ui, &before);
                            ui.set_av_worn_count(before.len() as i32);
                            bridge::report(&ui, e);
                        }
                    }
                },
            );
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_av_wear_outfit(move |id| {
            let ui = weak.unwrap();
            let Ok(outfit_id) = id.parse::<i64>() else { return };
            ui.set_av_busy(true);

            let client = app.client.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs3 = imgs2.clone();

            bridge2.call_res(
                move || async move { rojoin_roblox::avatar::wear_outfit(&client, outfit_id).await },
                move |ui, result| {
                    ui.set_av_busy(false);
                    match result {
                        Ok(ids) => {
                            *app2.worn.lock().unwrap() = ids.clone();
                            *app2.worn_write.lock().unwrap() =
                                Some((std::time::Instant::now(), ids.clone()));
                            apply_worn_flags(&ui, &ids);
                            ui.set_av_worn_count(ids.len() as i32);
                            load_avatar(&ui, &app2, &bridge3, &imgs3);
                        }
                        Err(e) => bridge::report(&ui, e),
                    }
                },
            );
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_av_save_outfit(move || {
            let ui = weak.unwrap();
            ui.set_av_busy(true);

            let client = app.client.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs3 = imgs2.clone();
            let name = format!("RoJoin {}", chrono::Local::now().format("%d %b %H:%M"));

            bridge2.call_res(
                move || async move {
                    let current = rojoin_roblox::avatar::mine(&client).await?;
                    rojoin_roblox::avatar::save_outfit(&client, &name, &current).await
                },
                move |ui, result| {
                    ui.set_av_busy(false);
                    match result {
                        Ok(()) => load_avatar(&ui, &app2, &bridge3, &imgs3),
                        Err(e) => bridge::report(&ui, e),
                    }
                },
            );
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_av_set_type(move |r15| {
            let ui = weak.unwrap();
            if ui.get_av_is_r15() == r15 {
                return;
            }
            ui.set_av_is_r15(r15);
            ui.set_av_busy(true);

            let client = app.client.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs3 = imgs2.clone();

            bridge2.call_res(
                move || async move { rojoin_roblox::avatar::set_avatar_type(&client, r15).await },
                move |ui, result| {
                    ui.set_av_busy(false);
                    match result {
                        // Body type does not change the worn list, so there is
                        // nothing to protect from a stale read here.
                        Ok(()) => refresh_avatar_render(&ui, &app2, &bridge3, &imgs3),
                        Err(e) => {
                            ui.set_av_is_r15(!r15);
                            bridge::report(&ui, e);
                        }
                    }
                },
            );
        });
    }
}

/// Re-tick the worn markers across the wardrobe grid and the wearing list.
/// Bring the "wearing" panel in line with a new worn list.
///
/// `apply_worn_flags` only ticks the wardrobe grid, so without this the panel
/// kept showing an item after it was taken off and omitted one just put on —
/// until a full reload happened to run. Additions borrow their name and
/// thumbnail from the wardrobe row that was clicked, so nothing has to be
/// refetched.
fn sync_worn_list(ui: &MainWindow, worn: &[i64]) {
    use slint::Model;

    let items = ui.get_av_items();
    let current = ui.get_av_worn();

    let mut rows: Vec<DetailItem> = current
        .iter()
        .filter(|r| r.id.parse::<i64>().map(|id| worn.contains(&id)).unwrap_or(false))
        .collect();

    for id in worn {
        let key = id.to_string();
        if rows.iter().any(|r| r.id == key) {
            continue;
        }
        // Pull the display details from the wardrobe entry for the same asset.
        if let Some(item) = items.iter().find(|i| i.id == key) {
            rows.push(DetailItem {
                id: key.into(),
                name: item.name.clone(),
                subtitle: item.category.clone(),
                thumb: item.thumb.clone(),
                kind: 7,
            });
        }
    }

    ui.set_av_worn(ad::model(rows));
}

fn apply_worn_flags(ui: &MainWindow, worn: &[i64]) {
    use slint::Model;
    let items = ui.get_av_items();
    for i in 0..items.row_count() {
        if let Some(mut row) = items.row_data(i) {
            let is_worn = row.id.parse::<i64>().map(|id| worn.contains(&id)).unwrap_or(false);
            if row.worn != is_worn {
                row.worn = is_worn;
                items.set_row_data(i, row);
            }
        }
    }
}

fn load_avatar(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let me = *app.me.lock().unwrap();
    if me == 0 {
        return;
    }

    ui.set_av_body_loading(true);
    let client = app.client.clone();
    let app2 = app.clone();
    let imgs2 = imgs.clone();
    let bridge2 = bridge.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move {
            let av = rojoin_roblox::avatar::mine(&client).await?;
            let outfits = or_default("outfits", rojoin_roblox::avatar::outfits(&client, me, 24).await);
            let body = thumbnails::avatars(&client, &[me])
                .await
                .ok()
                .and_then(|m| m.get(&me).cloned());

            let worn_ids = av.worn_ids();
            let worn_thumbs = or_default("item thumbnails", thumbnails::assets(&client, &worn_ids).await);
            let outfit_ids: Vec<i64> = outfits.iter().map(|o| o.id).collect();
            let outfit_thumbs = or_default("outfit thumbnails", thumbnails::outfits(&client, &outfit_ids).await);

            Ok(AvatarLoad { av, outfits, body, worn_thumbs, outfit_thumbs })
        },
        move |ui, result| {
            // Cleared before the staleness check: an early return here used to
            // leave the skeleton spinning for the rest of the session.
            ui.set_av_body_loading(false);
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }

            let load = match result {
                Ok(l) => l,
                Err(e) => return bridge::report(&ui, e),
            };

            let mut worn_ids = load.av.worn_ids();

            // Our own recent write beats a contradicting read: we know what we
            // sent, and Roblox may not have caught up yet.
            if let Some((at, ours)) = app2.worn_write.lock().unwrap().clone() {
                if at.elapsed() < std::time::Duration::from_secs(15) && ours != worn_ids {
                    tracing::debug!(
                        server = worn_ids.len(),
                        ours = ours.len(),
                        "ignoring a stale avatar read-back"
                    );
                    worn_ids = ours;
                }
            }

            *app2.worn.lock().unwrap() = worn_ids.clone();
            ui.set_av_worn_count(worn_ids.len() as i32);
            ui.set_av_is_r15(load.av.player_avatar_type != "R6");

            let worn_rows: Vec<DetailItem> = load
                .av
                .assets
                .iter()
                .map(|a| DetailItem {
                    id: a.id.to_string().into(),
                    name: a.name.clone().into(),
                    subtitle: a.asset_type.name.clone().into(),
                    thumb: slint::Image::default(),
                    kind: 7,
                })
                .collect();
            ui.set_av_worn(ad::model(worn_rows));

            for (i, a) in load.av.assets.iter().enumerate() {
                if let Some(url) = load.worn_thumbs.get(&a.id).cloned() {
                    let id = a.id.to_string();
                    imgs2.load(&url, move |ui, img| set_item_thumb(&ui.get_av_worn(), i, &id, img));
                }
            }

            let outfit_rows: Vec<DetailItem> = load
                .outfits
                .iter()
                .map(|o| DetailItem {
                    id: o.id.to_string().into(),
                    name: o.name.clone().into(),
                    subtitle: slint::SharedString::default(),
                    thumb: slint::Image::default(),
                    kind: 9,
                })
                .collect();
            ui.set_av_outfits(ad::model(outfit_rows));

            for (i, o) in load.outfits.iter().enumerate() {
                if let Some(url) = load.outfit_thumbs.get(&o.id).cloned() {
                    let id = o.id.to_string();
                    imgs2.load(&url, move |ui, img| set_item_thumb(&ui.get_av_outfits(), i, &id, img));
                }
            }

            if let Some(url) = load.body {
                imgs2.load(&url, |ui, img| ui.set_av_body(img));
            }

            apply_worn_flags(&ui, &worn_ids);
            load_wardrobe(&ui, &app2, &bridge2, &imgs2);
        },
    );
}

struct AvatarLoad {
    av: rojoin_roblox::avatar::Avatar,
    outfits: Vec<rojoin_roblox::avatar::Outfit>,
    body: Option<String>,
    worn_thumbs: std::collections::HashMap<i64, String>,
    outfit_thumbs: std::collections::HashMap<i64, String>,
}

/// The owned items for the selected category.
fn load_wardrobe(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let me = *app.me.lock().unwrap();
    if me == 0 {
        return;
    }

    let index = ui.get_av_category().max(0) as usize;
    let Some((category, _)) = rojoin_roblox::avatar::CATEGORIES.get(index) else { return };
    let category = category.to_string();

    ui.set_av_items_loading(true);
    ui.set_av_items(ad::model(Vec::new()));

    let client = app.client.clone();
    let imgs2 = imgs.clone();
    let worn = app.worn.lock().unwrap().clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move {
            let items =
                rojoin_roblox::avatar::inventory_for_category(&client, me, &category, 100).await?;
            let ids: Vec<i64> = items.iter().map(|i| i.asset_id).collect();
            let thumbs = or_default("item thumbnails", thumbnails::assets(&client, &ids).await);
            Ok((items, thumbs))
        },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_av_items_loading(false);

            let (items, thumbs) = match result {
                Ok(v) => v,
                Err(e) => return bridge::report(&ui, e),
            };

            let rows: Vec<WearItem> = items
                .iter()
                .map(|i| WearItem {
                    id: i.asset_id.to_string().into(),
                    name: i.asset_name.clone().into(),
                    category: slint::SharedString::default(),
                    thumb: slint::Image::default(),
                    worn: worn.contains(&i.asset_id),
                })
                .collect();
            ui.set_av_items(ad::model(rows));

            for (idx, item) in items.iter().enumerate() {
                if let Some(url) = thumbs.get(&item.asset_id).cloned() {
                    let id = item.asset_id.to_string();
                    imgs2.load(&url, move |ui, img| set_wear_thumb(&ui.get_av_items(), idx, &id, img));
                }
            }
        },
    );
}

/// Ask Roblox to re-render the avatar image after a change, then reload it.
fn refresh_avatar_render(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let me = *app.me.lock().unwrap();
    let client = app.client.clone();
    let imgs2 = imgs.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    // Show the skeleton while Roblox redraws, so the wait reads as progress
    // rather than as a view that has quietly given up.
    ui.set_av_body_loading(true);

    bridge.call_res(
        move || async move {
            Ok::<_, rojoin_roblox::Error>(thumbnails::avatar_when_ready(&client, me).await)
        },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_av_body_loading(false);
            if let Ok(Some(url)) = result {
                imgs2.load(&url, |ui, img| ui.set_av_body(img));
            }
        },
    );
}

fn wire_macros(ui: &MainWindow, app: &Arc<App>) {
    let available = rojoin_macro::backend::available();
    ui.set_macro_available(available);
    ui.set_macro_hint(
        rojoin_macro::backend::permission_hint()
            .unwrap_or_default()
            .into(),
    );
    ui.set_macro_backend(if cfg!(windows) { "SendInput".into() } else { "uinput".into() });

    {
        let mut macros = app.macros.lock().unwrap();
        let saved = app.config.lock().unwrap().settings.macros.clone();
        *macros = if saved.is_empty() {
            rojoin_macro::presets::all()
        } else {
            saved
        };
    }
    render_macros(ui, app);

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_macro_toggle_run(move |id| {
            let ui = weak.unwrap();
            let Some(engine) = app.engine.lock().unwrap().clone() else {
                tracing::warn!("no macro engine available");
                return;
            };

            if engine.is_running(id.as_str()) {
                engine.stop(id.as_str());
            } else {
                let macros = app.macros.lock().unwrap();
                if let Some(mac) = macros.iter().find(|m| m.id == id.as_str()) {
                    engine.start(mac);
                }
            }
            render_macros(&ui, &app);
        });
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_macro_toggle_enabled(move |id| {
            let ui = weak.unwrap();
            {
                let mut macros = app.macros.lock().unwrap();
                if let Some(mac) = macros.iter_mut().find(|m| m.id == id.as_str()) {
                    mac.enabled = !mac.enabled;
                    if !mac.enabled {
                        if let Some(engine) = app.engine.lock().unwrap().as_ref() {
                            engine.stop(&mac.id);
                        }
                    }
                }
            }
            persist_macros(&app);
            render_macros(&ui, &app);
        });
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_macro_stop_all(move || {
            let ui = weak.unwrap();
            if let Some(engine) = app.engine.lock().unwrap().as_ref() {
                engine.stop_all();
            }
            render_macros(&ui, &app);
        });
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_macro_rebind(move |id| {
            let ui = weak.unwrap();
            *app.editing.lock().unwrap() = Some(id.to_string());
            render_editor(&ui, &app);
            ui.set_editor_open(true);
            ui.set_editor_capturing(true);
            app.capturing.store(true, Ordering::SeqCst);
        });
    }

    wire_editor(ui, &app);

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_macro_open_credit(|| {});

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_macro_export(move || {
            let ui = weak.unwrap();
            let macros = app.macros.lock().unwrap().clone();
            let bundle = rojoin_macro::MacroBundle::new(macros);
            let path = rojoin_store::config_dir().join("macros-export.json");

            let status = match bundle.to_json().and_then(|json| {
                std::fs::create_dir_all(path.parent().unwrap_or(&path))
                    .and_then(|_| std::fs::write(&path, json))
                    .map_err(|e| rojoin_macro::Error::Input(e.to_string()))
            }) {
                Ok(()) => format!("Exported to {}", path.display()),
                Err(e) => format!("Export failed: {e}"),
            };
            tracing::info!(%status);
            ui.set_macro_io_status(status.into());
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_macro_import(move || {
            let ui = weak.unwrap();
            let dir = rojoin_store::config_dir();
            let candidates = [dir.join("macros-import.json"), dir.join("macros-export.json")];

            let Some(path) = candidates.iter().find(|p| p.exists()) else {
                ui.set_macro_io_status(
                    format!("Put a macros-import.json in {}", dir.display()).into(),
                );
                return;
            };

            let status = match std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    rojoin_macro::MacroBundle::from_json(&text).map_err(|e| e.to_string())
                }) {
                Ok(bundle) => {
                    let added = {
                        let mut macros = app.macros.lock().unwrap();
                        bundle.merge_into(&mut macros)
                    };
                    persist_macros(&app);
                    render_macros(&ui, &app);
                    if added == 1 {
                        "Imported 1 macro".to_string()
                    } else {
                        format!("Imported {added} macros")
                    }
                }
                Err(e) => format!("Import failed: {e}"),
            };
            tracing::info!(%status);
            ui.set_macro_io_status(status.into());
        });
    }
    ui.on_macro_edit(move |id| {
            let ui = weak.unwrap();
            *app.editing.lock().unwrap() = Some(id.to_string());
            ui.set_editor_capturing(false);
            render_editor(&ui, &app);
            ui.set_editor_open(true);
        });
    }

    if available {
        match rojoin_macro::Engine::new() {
            Ok(engine) => {
                *app.engine.lock().unwrap() = Some(Arc::new(engine));
                spawn_hotkey_listener(ui, app);
            }
            Err(e) => {
                tracing::warn!(error = %e, "macro engine unavailable");
                ui.set_macro_available(false);
                ui.set_macro_hint(format!("{e}").into());
            }
        }
    }
}

/// Fallback panic key when the configured one cannot be parsed.
const DEFAULT_PANIC_KEY: rojoin_macro::Key = rojoin_macro::Key::F8;

/// Watch for hotkeys system-wide and drive the engine from them.
///
/// Without this the macro tab would need you to alt-tab out of the game to
/// press Start, which defeats the point.
fn spawn_hotkey_listener(ui: &MainWindow, app: &Arc<App>) {
    use rojoin_macro::hotkeys::{HotkeyEvent, Listener};

    let (tx, rx) = std::sync::mpsc::channel();
    let Some(listener) = Listener::spawn(tx) else {
        ui.set_macro_hint(
            "Macros work, but RoJoin cannot watch for hotkeys — start them from this screen."
                .into(),
        );
        return;
    };
    *app.hotkeys.lock().unwrap() = Some(listener);

    let app = app.clone();
    let weak = ui.as_weak();

    std::thread::spawn(move || {
        let mut held: std::collections::HashMap<rojoin_macro::Key, Vec<String>> =
            std::collections::HashMap::new();

        while let Ok(event) = rx.recv() {
            let Some(engine) = app.engine.lock().unwrap().clone() else { continue };

            if app.binding_panic.load(Ordering::SeqCst) {
                if let HotkeyEvent::Pressed(key) = event {
                    app.binding_panic.store(false, Ordering::SeqCst);
                    if key != rojoin_macro::Key::Escape {
                        let mut cfg = app.config.lock().unwrap();
                        cfg.settings.panic_key = key.label().to_string();
                        let _ = cfg.save();
                    }
                    let app2 = app.clone();
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_binding_panic(false);
                        render_settings(&ui, &app2);
                    });
                }
                continue;
            }

            if app.capturing.load(Ordering::SeqCst) {
                if let HotkeyEvent::Pressed(key) = event {
                    app.capturing.store(false, Ordering::SeqCst);
                    {
                        let id = app.editing.lock().unwrap().clone();
                        if let Some(id) = id {
                            let mut macros = app.macros.lock().unwrap();
                            let bind = (key != rojoin_macro::Key::Escape).then_some(key);
                            for m in macros.iter_mut() {
                                if m.hotkey == bind && m.id != id {
                                    m.hotkey = None;
                                }
                                if m.id == id {
                                    m.hotkey = bind;
                                }
                            }
                        }
                    }
                    persist_macros(&app);

                    let app2 = app.clone();
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_editor_capturing(false);
                        render_editor(&ui, &app2);
                        render_macros(&ui, &app2);
                    });
                }
                continue;
            }

            let (enabled, gate_on_focus, panic_key) = {
                let cfg = app.config.lock().unwrap();
                (
                    cfg.settings.macros_enabled,
                    cfg.settings.macros_only_when_focused,
                    rojoin_macro::Key::from_label(&cfg.settings.panic_key)
                        .unwrap_or(DEFAULT_PANIC_KEY),
                )
            };

            if event == HotkeyEvent::Pressed(panic_key) {
                engine.stop_all();
                held.clear();
                tracing::info!("panic key: stopped all macros");
                let app2 = app.clone();
                let _ = weak.upgrade_in_event_loop(move |ui| render_macros(&ui, &app2));
                continue;
            }

            if !enabled {
                continue;
            }

            if gate_on_focus && !rojoin_macro::focus::current().allows_macros() {
                if let HotkeyEvent::Released(key) = event {
                    if let Some(ids) = held.remove(&key) {
                        for id in ids {
                            engine.stop(&id);
                        }
                    }
                }
                continue;
            }

            match event {
                HotkeyEvent::Pressed(key) => {
                    let macros = app.macros.lock().unwrap().clone();
                    for mac in macros.iter().filter(|m| m.enabled && m.hotkey == Some(key)) {
                        match mac.mode {
                            rojoin_macro::Mode::Hold => {
                                if engine.start(mac) {
                                    held.entry(key).or_default().push(mac.id.clone());
                                }
                            }
                            rojoin_macro::Mode::Toggle => {
                                if engine.is_running(&mac.id) {
                                    engine.stop(&mac.id);
                                } else {
                                    engine.start(mac);
                                }
                            }
                            rojoin_macro::Mode::Once => {
                                engine.start(mac);
                            }
                        }
                    }
                }

                HotkeyEvent::Released(key) => {
                    if let Some(ids) = held.remove(&key) {
                        for id in ids {
                            engine.stop(&id);
                        }
                    }
                }
            }

            let app2 = app.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| render_macros(&ui, &app2));
        }
    });
}

fn wire_editor(ui: &MainWindow, app: &Arc<App>) {
    ui.set_editor_keys(ad::strings(
        rojoin_macro::Key::ALL.iter().map(|k| k.label().to_string()).collect(),
    ));

    macro_rules! edit {
        ($hook:ident, |$mac:ident $(, $arg:ident : $ty:ty)*| $body:block) => {{
            let app = app.clone();
            let weak = ui.as_weak();
            ui.$hook(move |$($arg: $ty),*| {
                let ui = weak.unwrap();
                {
                    let Some(id) = app.editing.lock().unwrap().clone() else { return };
                    let mut macros = app.macros.lock().unwrap();
                    let Some($mac) = macros.iter_mut().find(|m| m.id == id) else { return };
                    $body
                }
                persist_macros(&app);
                render_editor(&ui, &app);
                render_macros(&ui, &app);
            });
        }};
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_editor_close(move || {
            let ui = weak.unwrap();
            ui.set_editor_open(false);
            ui.set_editor_capturing(false);
            app.capturing.store(false, Ordering::SeqCst);
            *app.editing.lock().unwrap() = None;
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_editor_capture(move || {
            let ui = weak.unwrap();
            let on = !ui.get_editor_capturing();
            ui.set_editor_capturing(on);
            app.capturing.store(on, Ordering::SeqCst);
        });
    }

    edit!(on_editor_set_mode, |m, mode: i32| {
        m.mode = match mode {
            0 => rojoin_macro::Mode::Once,
            1 => rojoin_macro::Mode::Hold,
            _ => rojoin_macro::Mode::Toggle,
        };
    });
    edit!(on_editor_set_gap, |m, gap: i32| {
        m.cycle_gap_ms = gap.max(0) as u64;
    });
    edit!(on_editor_set_amount, |m, i: i32, v: i32| {
        if let Some(step) = m.steps.get_mut(i.max(0) as usize) {
            let v = v.max(1) as u64;
            match step {
                rojoin_macro::Step::Tap { hold_ms, .. } => *hold_ms = v,
                rojoin_macro::Step::Wait { ms } => *ms = v,
                rojoin_macro::Step::Freeze { ms } => {
                    *ms = rojoin_macro::process::clamp_freeze(v)
                }
                _ => {}
            }
        }
    });
    edit!(on_editor_set_delta, |m, i: i32, dx: i32, dy: i32| {
        if let Some(rojoin_macro::Step::MouseMove { dx: x, dy: y }) =
            m.steps.get_mut(i.max(0) as usize)
        {
            *x = dx;
            *y = dy;
        }
    });
    edit!(on_editor_set_key, |m, i: i32, key: slint::SharedString| {
        let Some(k) = rojoin_macro::Key::from_label(key.as_str()) else { return };
        if let Some(step) = m.steps.get_mut(i.max(0) as usize) {
            match step {
                rojoin_macro::Step::KeyDown { key }
                | rojoin_macro::Step::KeyUp { key }
                | rojoin_macro::Step::Tap { key, .. } => *key = k,
                _ => {}
            }
        }
    });
    edit!(on_editor_move, |m, i: i32, delta: i32| {
        let from = i.max(0) as usize;
        let to = (i + delta).max(0) as usize;
        if from < m.steps.len() && to < m.steps.len() && from != to {
            m.steps.swap(from, to);
        }
    });
    edit!(on_editor_remove, |m, i: i32| {
        let idx = i.max(0) as usize;
        if idx < m.steps.len() {
            m.steps.remove(idx);
        }
    });
    edit!(on_editor_add, |m, kind: i32| {
        use rojoin_macro::{Key, MouseButton, Step};
        m.steps.push(match kind {
            0 => Step::KeyDown { key: Key::W },
            1 => Step::KeyUp { key: Key::W },
            2 => Step::Tap { key: Key::Space, hold_ms: 25 },
            3 => Step::Wait { ms: 50 },
            4 => Step::MouseDown { button: MouseButton::Left },
            5 => Step::MouseUp { button: MouseButton::Left },
            6 => Step::MouseMove { dx: 5, dy: 0 },
            _ => Step::Freeze { ms: 250 },
        });
    });
    edit!(on_editor_reset, |m| {
        if let Some(preset) = rojoin_macro::presets::all().into_iter().find(|p| p.id == m.id) {
            let hotkey = m.hotkey;
            *m = preset;
            m.hotkey = hotkey;
        }
    });
}

/// Rebuild the editor's view of the macro being edited.
fn render_editor(ui: &MainWindow, app: &Arc<App>) {
    use rojoin_macro::{MouseButton, Step};

    let Some(id) = app.editing.lock().unwrap().clone() else { return };
    let macros = app.macros.lock().unwrap();
    let Some(mac) = macros.iter().find(|m| m.id == id) else { return };

    let button_name = |b: &MouseButton| match b {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    };

    let rows: Vec<MacroStepRow> = mac
        .steps
        .iter()
        .map(|st| match st {
            Step::KeyDown { key } => MacroStepRow {
                kind: 0,
                label: "Hold key".into(),
                key: key.label().into(),
                amount: 0,
                dx: 0,
                dy: 0,
                has_key: true,
                has_amount: false,
                has_delta: false,
            },
            Step::KeyUp { key } => MacroStepRow {
                kind: 1,
                label: "Release key".into(),
                key: key.label().into(),
                amount: 0,
                dx: 0,
                dy: 0,
                has_key: true,
                has_amount: false,
                has_delta: false,
            },
            Step::Tap { key, hold_ms } => MacroStepRow {
                kind: 2,
                label: "Tap key, hold".into(),
                key: key.label().into(),
                amount: (*hold_ms).min(i32::MAX as u64) as i32,
                dx: 0,
                dy: 0,
                has_key: true,
                has_amount: true,
                has_delta: false,
            },
            Step::Wait { ms } => MacroStepRow {
                kind: 3,
                label: "Wait".into(),
                key: slint::SharedString::default(),
                amount: (*ms).min(i32::MAX as u64) as i32,
                dx: 0,
                dy: 0,
                has_key: false,
                has_amount: true,
                has_delta: false,
            },
            Step::MouseDown { button } => MacroStepRow {
                kind: 4,
                label: format!("Press {} click", button_name(button)).into(),
                key: slint::SharedString::default(),
                amount: 0,
                dx: 0,
                dy: 0,
                has_key: false,
                has_amount: false,
                has_delta: false,
            },
            Step::MouseUp { button } => MacroStepRow {
                kind: 5,
                label: format!("Release {} click", button_name(button)).into(),
                key: slint::SharedString::default(),
                amount: 0,
                dx: 0,
                dy: 0,
                has_key: false,
                has_amount: false,
                has_delta: false,
            },
            Step::Freeze { ms } => MacroStepRow {
                kind: 7,
                label: "Freeze game for".into(),
                key: slint::SharedString::default(),
                amount: (*ms).min(i32::MAX as u64) as i32,
                dx: 0,
                dy: 0,
                has_key: false,
                has_amount: true,
                has_delta: false,
            },
            Step::MouseMove { dx, dy } => MacroStepRow {
                kind: 6,
                label: "Move mouse".into(),
                key: slint::SharedString::default(),
                amount: 0,
                dx: *dx,
                dy: *dy,
                has_key: false,
                has_amount: false,
                has_delta: true,
            },
        })
        .collect();

    ui.set_editor_title(mac.name.clone().into());
    ui.set_editor_mode(match mac.mode {
        rojoin_macro::Mode::Once => 0,
        rojoin_macro::Mode::Hold => 1,
        rojoin_macro::Mode::Toggle => 2,
    });
    ui.set_editor_gap(mac.cycle_gap_ms.min(i32::MAX as u64) as i32);
    ui.set_editor_hotkey(mac.hotkey.map(|k| k.label().to_string()).unwrap_or_default().into());
    ui.set_editor_steps(ad::model(rows));
}

fn render_macros(ui: &MainWindow, app: &Arc<App>) {
    let engine = app.engine.lock().unwrap().clone();
    let macros = app.macros.lock().unwrap();

    let rows: Vec<MacroRow> = macros
        .iter()
        .map(|m| MacroRow {
            id: m.id.clone().into(),
            name: m.name.clone().into(),
            description: m.description.clone().into(),
            hotkey: m.hotkey.map(|k| k.label().to_string()).unwrap_or_default().into(),
            mode: match m.mode {
                rojoin_macro::Mode::Once => "Once",
                rojoin_macro::Mode::Hold => "While held",
                rojoin_macro::Mode::Toggle => "Toggle",
            }
            .into(),
            enabled: m.enabled,
            running: engine.as_ref().map(|e| e.is_running(&m.id)).unwrap_or(false),
            cycle: format!("{}ms cycle", m.cycle_ms()).into(),
            summary: format!("{} steps", m.steps.len()).into(),
        })
        .collect();

    ui.set_macro_active(engine.map(|e| e.active_count() as i32).unwrap_or(0));
    ui.set_macros(ad::model(rows));
}

fn persist_macros(app: &Arc<App>) {
    let macros = app.macros.lock().unwrap().clone();
    let mut cfg = app.config.lock().unwrap();
    cfg.settings.macros = macros;
    if let Err(e) = cfg.save() {
        tracing::error!(error = %e, "could not save macros");
    }
}

/// Section index for Settings. Deliberately past the end of NAV — Settings is
/// reached by the gear, not by a sidebar row.
const SETTINGS_SECTION: i32 = 6;

fn wire_settings(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    ui.set_data_dir(rojoin_store::config_dir().to_string_lossy().to_string().into());

    {
        let weak = ui.as_weak();
        ui.on_open_settings(move || {
            let ui = weak.unwrap();
            ui.set_view_kind(0);
            ui.set_can_back(false);
            ui.set_section(SETTINGS_SECTION);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_set_dark(move |dark| {
            let ui = weak.unwrap();
            ui.set_dark(dark);
            ui.global::<Theme>().set_dark(dark);
            let mut cfg = app.config.lock().unwrap();
            cfg.settings.dark = dark;
            let _ = cfg.save();
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_set_notify_requests(move |on| {
            let ui = weak.unwrap();
            ui.set_notify_requests(on);
            let mut cfg = app.config.lock().unwrap();
            cfg.settings.notify_friend_requests = on;
            let _ = cfg.save();
        });
    }
    {
        let app = app.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_unsub_friend(move |id| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.settings.notify_friends.retain(|f| *f != id.as_str());
                let _ = cfg.save();
            }
            render_settings(&ui, &app);
            recompute_friends(&ui, &app, &imgs2);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_unsub_game(move |id| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.settings.notify_games.retain(|g| *g != id.as_str());
                let _ = cfg.save();
            }
            render_settings(&ui, &app);
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_set_link_handler(move |on| {
            let ui = weak.unwrap();
            match linkhandler::set_registered(on) {
                Ok(()) => ui.set_link_handler(linkhandler::is_registered()),
                Err(e) => {
                    tracing::error!(error = %e, "could not change the link handler");
                    ui.set_link_handler(linkhandler::is_registered());
                }
            }
            let _ = &app;
        });
    }
    {
        ui.on_open_data_dir(move || {
            let dir = rojoin_store::config_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = webbrowser::open(&format!("file://{}", dir.display()));
        });
    }
    {
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_check_update(move || {
            let ui = weak.unwrap();
            ui.set_checking_update(true);
            ui.set_update_status("Checking…".into());

            let weak2 = weak.clone();
            bridge2.spawn(async move {
                let status = updater::check().await;

                // Finding an update and then doing nothing with it is what the
                // button used to do: it reported "0.1.1 is available" and threw
                // the download URL away. Checking implies installing.
                let updater::Status::Available { version, url } = status else {
                    let _ = weak2.upgrade_in_event_loop(move |ui| {
                        ui.set_checking_update(false);
                        ui.set_update_status(status.message().into());
                    });
                    return;
                };

                let v = version.clone();
                let _ = weak2.upgrade_in_event_loop(move |ui| {
                    ui.set_update_status(format!("Downloading {v}…").into());
                });

                let result = updater::install(&url).await;
                let _ = weak2.upgrade_in_event_loop(move |ui| {
                    ui.set_checking_update(false);
                    match result {
                        Ok(_) => {
                            ui.set_update_status(
                                format!("Version {version} installed. Restart to use it.").into(),
                            );
                            toast(&ui, "Update installed — restart to apply");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "update install failed");
                            ui.set_update_status(format!("Could not install: {e}").into());
                        }
                    }
                });
            });
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_switch_account(move |id| {
            let ui = weak.unwrap();
            let Some(cookie) = secrets::load(id.as_str()) else {
                tracing::warn!(%id, "no stored cookie for that account");
                return;
            };

            {
                let mut cfg = app.config.lock().unwrap();
                cfg.active_account = Some(id.to_string());
                let _ = cfg.save();
            }

            SESSION_GEN.fetch_add(1, Ordering::SeqCst);
            imgs2.clear();

            let client = app.client.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs3 = imgs2.clone();
            let weak2 = weak.clone();

            bridge2.spawn(async move {
                client.set_cookie(Some(cookie)).await;
                let me = users::authenticated(&client).await;
                let _ = weak2.upgrade_in_event_loop(move |ui| match me {
                    Ok(me) => enter_app(&ui, &app2, &bridge3, &imgs3, &me.display_name, me.id),
                    Err(e) => bridge::report(&ui, e),
                });
            });
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_remove_account(move |id| {
            let ui = weak.unwrap();
            secrets::delete(id.as_str());
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.remove_account(id.as_str());
                let _ = cfg.save();
            }
            render_settings(&ui, &app);
            ui.set_accounts_count(app.config.lock().unwrap().accounts.len() as i32);
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_add_account(move || {
            let ui = weak.unwrap();
            ui.set_signed_in(false);
            ui.set_signin_phase(SignInPhase::Idle);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_open_my_profile(move || {
            let ui = weak.unwrap();
            let me = *app.me.lock().unwrap();
            if me == 0 {
                tracing::warn!("no signed-in user id yet; ignoring");
                return;
            }
            open_profile(&ui, &app, &bridge2, &imgs2, me);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_save_description(move |text| {
            let ui = weak.unwrap();
            if app.bio_busy.swap(true, Ordering::SeqCst) {
                return;
            }
            ui.set_saving_bio(true);
            ui.set_bio_status("".into());

            let client = app.client.clone();
            let app2 = app.clone();
            let body = text.to_string();

            bridge2.call_res(
                move || async move { users::set_description(&client, &body).await },
                move |ui, result| {
                    app2.bio_busy.store(false, Ordering::SeqCst);
                    ui.set_saving_bio(false);
                    match result {
                        Ok(()) => {
                            // Reflect it locally too, so leaving and coming back
                            // shows the new text without another round trip.
                            let mut p = ui.get_profile();
                            p.description = text.clone();
                            ui.set_profile(p);
                            ui.set_bio_status("Saved".into());
                        }
                        Err(e) => {
                            ui.set_bio_status(format!("Could not save: {e}").into());
                            bridge::report(&ui, e);
                        }
                    }
                },
            );
        });
    }
    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_open_account(move || {
            let ui = weak.unwrap();
            ui.set_view_kind(0);
            ui.set_can_back(false);
            ui.set_section(SETTINGS_SECTION);
            render_settings(&ui, &app);
        });
    }

    let dark = app.config.lock().unwrap().settings.dark;
    ui.set_dark(dark);
    ui.global::<Theme>().set_dark(dark);
    ui.set_link_handler(linkhandler::is_registered());
    render_settings(ui, app);
}

/// Everything on the Settings screen that is a simple persisted toggle.
fn wire_more_settings(ui: &MainWindow, app: &Arc<App>) {
    ui.set_focus_detectable(rojoin_macro::focus::is_detectable());
    ui.set_sections(ad::strings(
        NAV.iter().map(|(label, _)| (*label).to_string()).collect(),
    ));

    macro_rules! setting {
        ($hook:ident, $arg:ty, |$cfg:ident, $v:ident| $body:block) => {{
            let app = app.clone();
            let weak = ui.as_weak();
            ui.$hook(move |$v: $arg| {
                let ui = weak.unwrap();
                {
                    let mut $cfg = app.config.lock().unwrap();
                    $body
                    let _ = $cfg.save();
                }
                render_settings(&ui, &app);
            });
        }};
    }

    setting!(on_set_server_page, i32, |cfg, v| {
        cfg.settings.server_page_size = [10, 25, 50, 100][v.clamp(0, 3) as usize];
    });
    setting!(on_set_timeout_choice, i32, |cfg, v| {
        cfg.settings.request_timeout_secs = [10, 20, 45, 90][v.clamp(0, 3) as usize];
    });
    setting!(on_set_cache_choice, i32, |cfg, v| {
        cfg.settings.image_cache_size = [100, 400, 800, 2000][v.clamp(0, 3) as usize];
        images::set_capacity(cfg.image_cache_size() as usize);
    });
    setting!(on_set_verbose_logging, bool, |cfg, v| { cfg.settings.verbose_logging = v; });
    setting!(on_set_auto_update, bool, |cfg, v| { cfg.settings.auto_update = v; });

    setting!(on_set_macros_enabled, bool, |cfg, v| { cfg.settings.macros_enabled = v; });
    setting!(on_set_only_when_focused, bool, |cfg, v| {
        cfg.settings.macros_only_when_focused = v;
    });
    setting!(on_set_startup_section, i32, |cfg, v| { cfg.settings.startup_section = v; });
    setting!(on_set_presence_secs, i32, |cfg, v| {
        cfg.settings.presence_refresh_secs = v.max(0) as u32;
    });
    setting!(on_set_confirm_destructive, bool, |cfg, v| {
        cfg.settings.confirm_destructive = v;
    });

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_set_ui_scale(move |v| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.settings.ui_scale = v;
                let _ = cfg.save();
            }
            let scale = app.config.lock().unwrap().ui_scale();
            apply_ui_scale(&ui, scale);
            render_settings(&ui, &app);
        });
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_bind_panic_key(move || {
            let ui = weak.unwrap();
            let on = !ui.get_binding_panic();
            ui.set_binding_panic(on);
            app.binding_panic.store(on, Ordering::SeqCst);
        });
    }

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_remove_shortcut(move |id| {
            let ui = weak.unwrap();
            let Ok(place_id) = id.parse::<i64>() else { return };
            for entry in shortcut::list() {
                if entry.place_id == place_id {
                    if let Err(e) = shortcut::remove(&entry.path, place_id) {
                        ui.set_launch_error(e.into());
                    }
                }
            }
            render_settings(&ui, &app);
        });
    }
    ui.on_open_log(open_log_file);

    {
        let app = app.clone();
        let weak = ui.as_weak();
        ui.on_clear_caches(move || {
            let ui = weak.unwrap();
            {
                let mut names = app.names.lock().unwrap();
                names.users.clear();
                let _ = names.save();
            }
            if let Some(loader) = app.imgs.lock().unwrap().as_ref() {
                loader.clear();
            }
            tracing::info!("caches cleared");
            render_settings(&ui, &app);
        });
    }
}

fn render_settings(ui: &MainWindow, app: &Arc<App>) {
    let cfg = app.config.lock().unwrap();

    let rows: Vec<AccountRow> = cfg
        .accounts
        .iter()
        .map(|a| AccountRow {
            id: a.id.clone().into(),
            name: a.display_name.clone().into(),
            username: a.username.clone().into(),
            avatar: slint::Image::default(),
            active: cfg.active_account.as_deref() == Some(a.id.as_str()),
        })
        .collect();
    ui.set_accounts(ad::model(rows));
    ui.set_shortcuts(ad::model(
        shortcut::list()
            .into_iter()
            .map(|e| DetailItem {
                id: e.place_id.to_string().into(),
                name: e.name.into(),
                subtitle: Default::default(),
                thumb: slint::Image::default(),
                kind: 0,
            })
            .collect::<Vec<_>>(),
    ));
    ui.set_notify_requests(cfg.settings.notify_friend_requests);
    ui.set_macros_enabled(cfg.settings.macros_enabled);
    ui.set_only_when_focused(cfg.settings.macros_only_when_focused);
    ui.set_panic_key(cfg.settings.panic_key.clone().into());
    ui.set_ui_scale(cfg.ui_scale());
    ui.set_startup_section(cfg.settings.startup_section);
    ui.set_presence_secs(cfg.presence_refresh_secs() as i32);
    ui.set_confirm_destructive(cfg.settings.confirm_destructive);
    ui.set_server_page(match cfg.server_page_size() {
        10 => 0,
        25 => 1,
        50 => 2,
        _ => 3,
    });
    ui.set_timeout_choice(match cfg.request_timeout_secs() {
        10 => 0,
        20 => 1,
        45 => 2,
        _ => 3,
    });
    ui.set_cache_choice(match cfg.image_cache_size() {
        100 => 0,
        400 => 1,
        800 => 2,
        _ => 3,
    });
    ui.set_verbose_logging(cfg.settings.verbose_logging);
    ui.set_auto_update(cfg.settings.auto_update);

    // Account head shots. The stored `avatar_url` is only filled in at sign-in
    // and goes stale as soon as someone changes their avatar, so ask Roblox
    // rather than trusting whatever the config happens to hold.
    render_client_settings(ui);

    let ids: Vec<i64> = cfg.accounts.iter().filter_map(|a| a.id.parse().ok()).collect();
    let imgs = app.imgs.lock().unwrap().clone();
    if let (Some(bridge), Some(loader)) = (app.bridge.lock().unwrap().clone(), imgs) {
        if !ids.is_empty() {
            let client = app.client.clone();
            let order = ids.clone();
            bridge.call_res(
                move || async move { thumbnails::headshots(&client, &ids).await },
                move |_ui, result| {
                    let Ok(map) = result else { return };
                    for (i, id) in order.iter().enumerate() {
                        let Some(url) = map.get(id).cloned() else { continue };
                        loader.load(&url, move |ui, img| {
                            use slint::Model;
                            let model = ui.get_accounts();
                            if let Some(mut row) = model.row_data(i) {
                                row.avatar = img;
                                model.set_row_data(i, row);
                            }
                        });
                    }
                },
            );
        }
    }

    let cached = app.names.lock().unwrap().users.len();
    ui.set_cache_summary(
        format!("{cached} names remembered, so they never need re-fetching").into(),
    );

    let roster = app.roster.lock().unwrap();
    let friends: Vec<DetailItem> = cfg
        .settings
        .notify_friends
        .iter()
        .map(|id| {
            let name = roster
                .iter()
                .find(|f| f.id.to_string() == *id)
                .map(|f| f.name.clone())
                .unwrap_or_else(|| format!("User {id}"));
            DetailItem {
                id: id.clone().into(),
                name: name.into(),
                subtitle: slint::SharedString::default(),
                thumb: slint::Image::default(),
                kind: 8,
            }
        })
        .collect();
    ui.set_notify_friends_list(ad::model(friends));

    let games: Vec<DetailItem> = cfg
        .settings
        .notify_games
        .iter()
        .map(|id| DetailItem {
            id: id.clone().into(),
            name: format!("Game {id}").into(),
            subtitle: slint::SharedString::default(),
            thumb: slint::Image::default(),
            kind: 1,
        })
        .collect();
    ui.set_notify_games_list(ad::model(games));
}

/// Playtime stats, computed from local history.
/// Home layout and the pinned-games grid.
fn wire_home(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_toggle_home_section(move |kind| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.toggle_home_section(kind);
                let _ = cfg.save();
            }
            apply_home_sections(&ui, &app);
            load_pinned(&ui, &app, &bridge2, &imgs2);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_unpin_game(move |id| {
            let ui = weak.unwrap();
            {
                let mut cfg = app.config.lock().unwrap();
                cfg.toggle_pin(id.as_str());
                let _ = cfg.save();
            }
            load_pinned(&ui, &app, &bridge2, &imgs2);
        });
    }
}

/// Push the saved home layout into the five section flags.
fn apply_home_sections(ui: &MainWindow, app: &Arc<App>) {
    let on = app.config.lock().unwrap().home_sections();
    ui.set_show_friends(on.contains(&0));
    ui.set_show_hero(on.contains(&1));
    ui.set_show_pinned(on.contains(&2));
    ui.set_show_recent(on.contains(&3));
    ui.set_show_favorites(on.contains(&4));
}

/// Games the user pinned locally, shown on Home.
fn load_pinned(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let places: Vec<i64> = {
        let cfg = app.config.lock().unwrap();
        cfg.data()
            .map(|d| d.pins.iter().filter_map(|p| p.parse::<i64>().ok()).collect())
            .unwrap_or_default()
    };

    if places.is_empty() {
        ui.set_pinned(ad::model(Vec::new()));
        return;
    }

    let client = app.client.clone();
    let imgs2 = imgs.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move { fetch_by_places(&client, &places).await },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            let Ok(load) = result else { return };

            let tiles: Vec<GameTile> = load
                .details
                .iter()
                .map(|d| {
                    let v = load.votes.iter().find(|v| v.id == d.id);
                    ad::tile_from_detail(d, v, favorited_ids(&ui).contains(&d.id))
                })
                .collect();
            ui.set_pinned(ad::model(tiles));

            for (i, d) in load.details.iter().enumerate() {
                if let Some(url) = load.art.get(&d.id).cloned() {
                    let id = d.root_place_id.to_string();
                    imgs2.load(&url, move |ui, img| {
                        set_tile_thumb(&ui.get_pinned(), i, &id, img)
                    });
                }
            }
        },
    );
}

fn render_library(ui: &MainWindow, app: &Arc<App>) {
    let cfg = app.config.lock().unwrap();
    let Some(data) = cfg.data() else {
        ui.set_most_played(ad::model(Vec::new()));
        ui.set_total_playtime("0m".into());
        return;
    };

    let mut entries: Vec<_> = data.history.values().collect();
    entries.sort_by_key(|h| std::cmp::Reverse(h.playtime_secs));

    let total: u64 = entries.iter().map(|h| h.playtime_secs).sum();
    let launches: u32 = entries.iter().map(|h| h.launches).sum();
    let top = entries.first().map(|h| h.playtime_secs).unwrap_or(0);

    let stats: Vec<PlayStat> = entries
        .iter()
        .filter(|h| h.playtime_secs > 0)
        .take(10)
        .map(|h| PlayStat {
            id: h.place_id.clone().into(),
            name: if h.name.is_empty() {
                format!("Place {}", h.place_id).into()
            } else {
                h.name.clone().into()
            },
            value: ad::fmt_duration(h.playtime_secs).into(),
            fraction: if top > 0 { h.playtime_secs as f32 / top as f32 } else { 0.0 },
        })
        .collect();

    ui.set_most_played(ad::model(stats));
    ui.set_total_playtime(ad::fmt_duration(total).into());
    ui.set_games_played(entries.len() as i32);
    ui.set_total_launches(launches as i32);
}

fn push_view(ui: &MainWindow, app: &Arc<App>, kind: i32) {
    if ui.get_view_kind() == 0 {
        *app.return_section.lock().unwrap() = ui.get_section();
    }
    ui.set_view_kind(kind);
    ui.set_can_back(true);
}

/// Accept or decline the friend request open on the profile being viewed.
///
/// Records the answer the same way the Friends tab does, so the row does not
/// come back on the next refresh — Roblox keeps serving a handled request for
/// a while after it is answered.
fn answer_request(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, accept: bool) {
    let Ok(uid) = ui.get_profile().id.parse::<i64>() else { return };

    let mut p = ui.get_profile();
    p.has_incoming_request = false;
    p.is_friend = accept;
    ui.set_profile(p);

    app.handled_requests.lock().unwrap().insert(uid.to_string());
    {
        let mut cfg = app.config.lock().unwrap();
        cfg.remember_handled_request(uid.to_string());
        let _ = cfg.save();
    }
    drop_request_row(ui, uid);

    let client = app.client.clone();
    ui.set_profile_busy(true);
    bridge.call_res(
        move || async move {
            if accept {
                friends::accept(&client, uid).await
            } else {
                friends::decline(&client, uid).await
            }
        },
        move |ui, result| {
            ui.set_profile_busy(false);
            if let Err(e) = result {
                bridge::report(&ui, e);
            }
        },
    );
}

fn open_profile(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images, user_id: i64) {
    push_view(ui, app, 2);
    ui.set_profile_loading(true);
    ui.set_profile_tab(0);
    // Your own profile gets the editable About me; everyone else's stays read
    // only. Set before the fetch so the editor does not flash into view.
    ui.set_profile_is_me(user_id != 0 && user_id == *app.me.lock().unwrap());
    ui.set_bio_status(Default::default());
    ui.set_profile_groups(ad::model(Vec::new()));
    ui.set_profile_favorites(ad::model(Vec::new()));

    let client = app.client.clone();
    let imgs2 = imgs.clone();
    let me = *app.me.lock().unwrap();
    let friend_ids: Vec<i64> = app.roster.lock().unwrap().iter().map(|f| f.id).collect();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move { fetch_profile(&client, user_id).await },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_profile_loading(false);

            let load = match result {
                Ok(l) => l,
                Err(e) => return bridge::report(&ui, e),
            };

            let presence = load.presence.as_ref();
            let kind = presence.map(|p| p.kind).unwrap_or(friends::PresenceKind::Offline);

            ui.set_profile(ProfileData {
                id: load.user.id.to_string().into(),
                name: load.user.display_name.clone().into(),
                username: load.user.name.clone().into(),
                description: load.user.description.clone().unwrap_or_default().into(),
                joined: load
                    .user
                    .created
                    .as_deref()
                    .and_then(users::account_age_years)
                    .map(|y| format!("{y}y"))
                    .unwrap_or_else(|| "—".into())
                    .into(),
                also_known_as: load.previous_names.join(", ").into(),
                verified: load.user.has_verified_badge,
                friends: ad::compact(load.counts.friends).into(),
                followers: ad::compact(load.counts.followers).into(),
                following: ad::compact(load.counts.following).into(),
                presence: match kind {
                    friends::PresenceKind::Online => 1,
                    friends::PresenceKind::InGame => 2,
                    friends::PresenceKind::InStudio => 3,
                    _ => 0,
                },
                presence_label: presence
                    .map(|p| {
                        if p.location.is_empty() {
                            "Offline".to_string()
                        } else {
                            p.location.clone()
                        }
                    })
                    .unwrap_or_else(|| "Offline".into())
                    .into(),
                place_id: presence
                    .and_then(|p| p.place_id.or(p.root_place_id))
                    .map(|p| p.to_string())
                    .unwrap_or_default()
                    .into(),
                is_friend: friend_ids.contains(&user_id),
                // Answered from the requests already on screen rather than a
                // fresh call: the list is short and this runs on every profile.
                has_incoming_request: {
                    use slint::Model;
                    let key = user_id.to_string();
                    ui.get_requests_list().iter().any(|r| r.id == key)
                },
                is_self: user_id == me,
                avatar: slint::Image::default(),
            });

            if let Some(url) = load.avatar {
                imgs2.load(&url, |ui, img| {
                    let mut p = ui.get_profile();
                    p.avatar = img;
                    ui.set_profile(p);
                });
            }

            let group_rows: Vec<GroupRow> = load
                .groups
                .iter()
                .map(|m| GroupRow {
                    id: m.group.id.to_string().into(),
                    name: m.group.name.clone().into(),
                    role: m.role.name.clone().into(),
                    members: ad::compact(m.group.member_count).into(),
                    icon: slint::Image::default(),
                })
                .collect();
            ui.set_profile_groups(ad::model(group_rows));

            for (i, m) in load.groups.iter().enumerate() {
                if let Some(url) = load.group_icons.get(&m.group.id).cloned() {
                    imgs2.load(&url, move |ui, img| {
                        set_group_icon(&ui.get_profile_groups(), i, img)
                    });
                }
            }

            let favs: Vec<GameTile> = load
                .favorites
                .iter()
                .map(|d| ad::tile_from_detail(d, None, true))
                .collect();
            ui.set_profile_favorites(ad::model(favs));

            for (i, d) in load.favorites.iter().enumerate() {
                if let Some(url) = load.game_art.get(&d.id).cloned() {
                    let id = d.root_place_id.to_string();
                    imgs2.load(&url, move |ui, img| {
                        set_tile_thumb(&ui.get_profile_favorites(), i, &id, img)
                    });
                }
            }
        },
    );
}

struct ProfileLoad {
    user: rojoin_roblox::models::User,
    previous_names: Vec<String>,
    counts: friends::SocialCounts,
    presence: Option<friends::Presence>,
    groups: Vec<groups::Membership>,
    favorites: Vec<rojoin_roblox::models::GameDetail>,
    avatar: Option<String>,
    group_icons: std::collections::HashMap<i64, String>,
    game_art: std::collections::HashMap<i64, String>,
}

async fn fetch_profile(client: &Client, user_id: i64) -> rojoin_roblox::Result<ProfileLoad> {
    let user = users::get(client, user_id).await?;
    let previous_names = or_default("previous usernames", users::previous_usernames(client, user_id).await);
    let counts = friends::counts(client, user_id).await;
    let presence = friends::presence(client, &[user_id])
        .await
        .ok()
        .and_then(|v| v.into_iter().next());
    let groups = or_default("the user's groups", groups::of_user(client, user_id).await);
    let favorites = or_default("favourite games", games::user_favorites(client, user_id, 12).await);

    let avatar = thumbnails::avatars(client, &[user_id])
        .await
        .ok()
        .and_then(|m| m.get(&user_id).cloned());

    let group_ids: Vec<i64> = groups.iter().map(|m| m.group.id).collect();
    let group_icons = or_default("group icons", thumbnails::group_icons(client, &group_ids).await);

    let universe_ids: Vec<i64> = favorites.iter().map(|d| d.id).collect();
    let game_art = or_default("game art", thumbnails::game_art(client, &universe_ids).await);

    Ok(ProfileLoad {
        user,
        previous_names,
        counts,
        presence,
        groups,
        favorites,
        avatar,
        group_icons,
        game_art,
    })
}

fn open_group(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images, group_id: i64) {
    push_view(ui, app, 3);
    ui.set_group_loading(true);
    ui.set_group_games(ad::model(Vec::new()));

    let client = app.client.clone();
    let imgs2 = imgs.clone();
    let me = *app.me.lock().unwrap();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move {
            let group = groups::get(&client, group_id).await?;
            let mine = or_default("the user's groups", groups::of_user(&client, me).await);
            let membership = mine.into_iter().find(|m| m.group.id == group_id);
            let games_list = or_default("the group's games", games::group_games(&client, group_id, 12).await);

            let icon = thumbnails::group_icons(&client, &[group_id])
                .await
                .ok()
                .and_then(|m| m.get(&group_id).cloned());
            let owner_avatar = match group.owner.as_ref() {
                Some(o) => thumbnails::headshots(&client, &[o.user_id])
                    .await
                    .ok()
                    .and_then(|m| m.get(&o.user_id).cloned()),
                None => None,
            };
            let universe_ids: Vec<i64> = games_list.iter().map(|d| d.id).collect();
            let art = or_default("game art", thumbnails::game_art(&client, &universe_ids).await);

            Ok(GroupLoad { group, membership, games: games_list, icon, owner_avatar, art })
        },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_group_loading(false);

            let load = match result {
                Ok(l) => l,
                Err(e) => return bridge::report(&ui, e),
            };

            let owner = load.group.owner.clone().unwrap_or_default();
            let shout = load.group.shout.clone().unwrap_or_default();

            ui.set_group(GroupData {
                id: load.group.id.to_string().into(),
                name: load.group.name.clone().into(),
                description: load.group.description.clone().unwrap_or_default().into(),
                members: ad::compact(load.group.member_count).into(),
                role: load
                    .membership
                    .as_ref()
                    .map(|m| m.role.name.clone())
                    .unwrap_or_default()
                    .into(),
                shout: shout.body.clone().into(),
                shout_by: shout
                    .poster
                    .as_ref()
                    .map(|p| p.display_name.clone())
                    .unwrap_or_default()
                    .into(),
                owner: owner.display_name.clone().into(),
                owner_id: owner.user_id.to_string().into(),
                is_member: load.membership.is_some(),
                open_entry: load.group.public_entry_allowed,
                icon: slint::Image::default(),
                owner_avatar: slint::Image::default(),
            });

            if let Some(url) = load.icon {
                imgs2.load(&url, |ui, img| {
                    let mut g = ui.get_group();
                    g.icon = img;
                    ui.set_group(g);
                });
            }
            if let Some(url) = load.owner_avatar {
                imgs2.load(&url, |ui, img| {
                    let mut g = ui.get_group();
                    g.owner_avatar = img;
                    ui.set_group(g);
                });
            }

            let tiles: Vec<GameTile> = load
                .games
                .iter()
                .map(|d| ad::tile_from_detail(d, None, false))
                .collect();
            ui.set_group_games(ad::model(tiles));

            for (i, d) in load.games.iter().enumerate() {
                if let Some(url) = load.art.get(&d.id).cloned() {
                    let id = d.root_place_id.to_string();
                    imgs2.load(&url, move |ui, img| {
                        set_tile_thumb(&ui.get_group_games(), i, &id, img)
                    });
                }
            }
        },
    );
}

struct GroupLoad {
    group: groups::Group,
    membership: Option<groups::Membership>,
    games: Vec<rojoin_roblox::models::GameDetail>,
    icon: Option<String>,
    owner_avatar: Option<String>,
    art: std::collections::HashMap<i64, String>,
}

/// Universe ids currently sitting in the favourites model, so a tile built for
/// another grid can start with the right star instead of an empty one.
fn favorited_ids(ui: &MainWindow) -> std::collections::HashSet<i64> {
    use slint::Model;
    ui.get_favorites()
        .iter()
        .filter_map(|t| t.universe_id.parse::<i64>().ok())
        .collect()
}

/// The signed-in user's favourited games, for Home and the Library tab.
///
/// Nothing else fills that model, so without this both surfaces sit empty and
/// the star on a tile has nothing to toggle against.
fn load_favorites(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    let me = *app.me.lock().unwrap();
    if me == 0 {
        return;
    }

    let client = app.client.clone();
    let imgs2 = imgs.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move {
            let games = games::user_favorites(&client, me, 50).await?;
            let ids: Vec<i64> = games.iter().map(|g| g.id).collect();
            let art = or_default("game art", thumbnails::game_art(&client, &ids).await);
            Ok((games, art))
        },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            let Ok((games, art)) = result else { return };

            let tiles: Vec<GameTile> = games
                .iter()
                .map(|d| ad::tile_from_detail(d, None, true))
                .collect();
            ui.set_favorites(ad::model(tiles));

            // Now that the real answer is known, correct the star on every
            // other grid showing the same game.
            for d in &games {
                set_tile_favorited(&ui, &d.id.to_string(), true);
            }

            for (i, d) in games.iter().enumerate() {
                let Some(url) = art.get(&d.id).cloned() else { continue };
                let id = d.root_place_id.to_string();
                imgs2.load(&url, move |ui, img| {
                    set_tile_thumb(&ui.get_favorites(), i, &id, img)
                });
            }
        },
    );
}

fn load_home(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    ui.set_home_loading(true);

    let client = app.client.clone();
    let imgs = imgs.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move { fetch_home(&client).await },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_home_loading(false);

            let load = match result {
                Ok(load) => load,
                Err(e) => return bridge::report(&ui, e),
            };

            let tiles: Vec<GameTile> = load
                .details
                .iter()
                .map(|d| {
                    let v = load.votes.iter().find(|v| v.id == d.id);
                    ad::tile_from_detail(d, v, false)
                })
                .collect();

            match tiles.first().cloned() {
                Some(first) => {
                    ui.set_hero(first);
                    ui.set_has_hero(true);
                }
                // A new account has nothing to continue, and Roblox omits the
                // sort rather than sending an empty one.
                None => ui.set_has_hero(false),
            }
            ui.set_recent(ad::model(tiles));

            for (i, d) in load.details.iter().enumerate() {
                let Some(url) = load.art.get(&d.id).cloned() else { continue };
                let id = d.root_place_id.to_string();
                imgs.load(&url, move |ui, img| {
                    set_tile_thumb(&ui.get_recent(), i, &id, img)
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
            if let Some(target) = search::resolve_join_target(q.as_str()) {
                // A link that names a specific server or a VIP code is a join
                // instruction, not a browse one — honour it directly instead of
                // dropping the user on the game page to pick a server again.
                if target.job_id.is_some()
                    || target.access_code.is_some()
                    || target.link_code.is_some()
                {
                    let mut req = JoinRequest::place(target.place_id);
                    if let Some(job) = target.job_id {
                        req = req.server(job);
                    }
                    if let Some(code) = target.access_code {
                        req = req.reserved(code);
                    }
                    if let Some(code) = target.link_code {
                        req = req.private_link(code);
                    }
                    open_game(&ui, &app, &bridge2, &imgs2, target.place_id);
                    launch(&ui, &app, req);
                    return;
                }
                open_game(&ui, &app, &bridge2, &imgs2, target.place_id);
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
        ui.set_play_players(ad::model(Vec::new()));
        ui.set_play_groups(ad::model(Vec::new()));
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

    // People and groups run alongside the games request, so both need their own
    // handles before the games closure takes ownership of these.
    search_people(ui, app, bridge, imgs, &query, gen);

    let client = app.client.clone();
    let imgs = imgs.clone();
    let session = app.search_session.lock().unwrap().clone();
    let q2 = query.clone();

    bridge.call_res(
        move || async move {
            let page = search::games(&client, &q2, &session, None).await?;
            let ids: Vec<i64> = page.games.iter().map(|g| g.universe_id).collect();
            let art = or_default("game art", thumbnails::game_art(&client, &ids).await);
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
                            let id = g.root_place_id.to_string();
                            imgs.load(&url, move |ui, img| {
                                set_tile_thumb(&ui.get_play_games(), i, &id, img)
                            });
                        }
                    }
                }
                Err(e) => {
                    ui.set_play_games(ad::model(Vec::new()));
                    bridge::report(&ui, e);
                }
            }
        },
    );
}

/// People and groups matching the same query.
///
/// Kept off the games request on purpose: user search is a separate service
/// that rate-limits on its own schedule, and a throttle there should not cost
/// the game results that already came back.
fn search_people(
    ui: &MainWindow,
    app: &Arc<App>,
    bridge: &Arc<Bridge>,
    imgs: &Images,
    query: &str,
    gen: i64,
) {
    let client = app.client.clone();
    let imgs = imgs.clone();
    let q = query.to_string();

    bridge.call_res(
        move || async move {
            // Tolerated separately: user search and group search throttle on
            // their own schedules, and losing one should not cost the other.
            // Logged, because swallowing these silently once made a rejected
            // page size look exactly like "no such user".
            let users = search::users(&client, &q, 25)
                .await
                .inspect_err(|e| tracing::warn!(error = %e, "user search failed"))
                .unwrap_or_default();
            let groups = search::groups(&client, &q, 10)
                .await
                .inspect_err(|e| tracing::warn!(error = %e, "group search failed"))
                .unwrap_or_default();

            let user_ids: Vec<i64> = users.iter().map(|u| u.id).collect();
            let group_ids: Vec<i64> = groups.iter().map(|g| g.id).collect();

            let heads = or_default("avatars", thumbnails::headshots(&client, &user_ids).await);
            let icons = or_default("group icons", thumbnails::group_icons(&client, &group_ids).await);

            Ok::<_, rojoin_roblox::Error>((users, groups, heads, icons))
        },
        move |ui, result| {
            if gen != SEARCH_GEN.load(Ordering::SeqCst) {
                return;
            }
            let Ok((users, groups, heads, icons)) = result else { return };

            ui.set_play_players(ad::model(
                users
                    .iter()
                    .map(|u| DetailItem {
                        id: u.id.to_string().into(),
                        name: u.display_name.clone().into(),
                        subtitle: format!("@{}", u.name).into(),
                        thumb: slint::Image::default(),
                        kind: 0,
                    })
                    .collect::<Vec<_>>(),
            ));
            ui.set_play_groups(ad::model(
                groups
                    .iter()
                    .map(|g| DetailItem {
                        id: g.id.to_string().into(),
                        name: g.name.clone().into(),
                        subtitle: format!("{} members", ad::compact(g.member_count)).into(),
                        thumb: slint::Image::default(),
                        kind: 0,
                    })
                    .collect::<Vec<_>>(),
            ));

            for (i, u) in users.iter().enumerate() {
                let Some(url) = heads.get(&u.id).cloned() else { continue };
                let id = u.id.to_string();
                imgs.load(&url, move |ui, img| {
                    set_item_thumb(&ui.get_play_players(), i, &id, img)
                });
            }
            for (i, g) in groups.iter().enumerate() {
                let Some(url) = icons.get(&g.id).cloned() else { continue };
                let id = g.id.to_string();
                imgs.load(&url, move |ui, img| {
                    set_item_thumb(&ui.get_play_groups(), i, &id, img)
                });
            }
        },
    );
}

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
        let imgs2 = imgs.clone();
        ui.on_set_server_sort(move |sort| {
            fetch_servers(&weak.unwrap(), &app, &bridge2, &imgs2, sort);
        });
    }
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs = imgs.clone();
        let weak = ui.as_weak();
        ui.on_toggle_favorite(move |universe_id| {
            let ui = weak.unwrap();
            let Ok(uid) = universe_id.parse::<i64>() else { return };

            // The star appears both on the detail page and on every grid tile,
            // so flip whichever of the two the click came from — and flip the
            // same game everywhere else it happens to be on screen.
            let detail_matches = ui.get_game().universe_id == universe_id;
            let target = if detail_matches {
                !ui.get_game().favorited
            } else {
                !tile_favorited(&ui, &universe_id).unwrap_or(false)
            };

            if detail_matches {
                let mut g = ui.get_game();
                g.favorited = target;
                ui.set_game(g);
            }
            set_tile_favorited(&ui, &universe_id, target);

            let client = app.client.clone();
            let uni = universe_id.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs2 = imgs.clone();
            bridge2.call_res(
                move || async move { games::set_favorited(&client, uid, target).await },
                move |ui, result| {
                    if let Err(e) = result {
                        // Put the star back; the write did not land.
                        if ui.get_game().universe_id == uni {
                            let mut g = ui.get_game();
                            g.favorited = !target;
                            ui.set_game(g);
                        }
                        set_tile_favorited(&ui, &uni, !target);
                        bridge::report(&ui, e);
                        return;
                    }
                    // The favourites grid is a list, not a flag, so it has to
                    // be refetched rather than nudged.
                    load_favorites(&ui, &app2, &bridge3, &imgs2);
                },
            );
        });
    }
}

/// Every model that can be showing a game tile. The star has to stay in sync
/// across all of them, because the same game turns up in several at once.
fn tile_models(ui: &MainWindow) -> Vec<slint::ModelRc<GameTile>> {
    vec![
        ui.get_recent(),
        ui.get_favorites(),
        ui.get_pinned(),
        ui.get_play_games(),
        ui.get_related(),
        ui.get_profile_favorites(),
        ui.get_group_games(),
    ]
}

fn tile_favorited(ui: &MainWindow, universe_id: &str) -> Option<bool> {
    use slint::Model;
    tile_models(ui)
        .iter()
        .flat_map(|m| m.iter())
        .find(|t| t.universe_id == universe_id)
        .map(|t| t.favorited)
}

fn set_tile_favorited(ui: &MainWindow, universe_id: &str, favorited: bool) {
    use slint::Model;
    for model in tile_models(ui) {
        for i in 0..model.row_count() {
            let Some(mut tile) = model.row_data(i) else { continue };
            if tile.universe_id != universe_id {
                continue;
            }
            tile.favorited = favorited;
            model.set_row_data(i, tile);
        }
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
            let votes = or_default("ratings", games::votes(&client, &[universe_id]).await);
            let subs = games::sub_places(&client, universe_id, detail.root_place_id)
                .await
                .unwrap_or_default();
            let passes = or_default("game passes", games::game_passes(&client, universe_id).await);
            let badges = or_default("badges", games::badges(&client, universe_id).await);
            let icons = or_default("game icons", thumbnails::game_icons(&client, &[universe_id]).await);
            let art = or_default("game art", thumbnails::game_art(&client, &[universe_id]).await);

            let badge_ids: Vec<i64> = badges.iter().map(|b| b.id).collect();
            let badge_icons = or_default("badge icons", thumbnails::badges(&client, &badge_ids).await);
            let pass_ids: Vec<i64> = passes.iter().map(|p| p.id).collect();
            let pass_icons = or_default("game-pass icons", thumbnails::game_passes(&client, &pass_ids).await);

            Ok(GameLoad {
                universe_id,
                detail,
                votes: votes.into_iter().next(),
                subs,
                passes,
                badges,
                icon: icons.get(&universe_id).cloned(),
                hero: art.get(&universe_id).cloned(),
                badge_icons,
                pass_icons,
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

                    for (i, b) in load.badges.iter().enumerate() {
                        if let Some(url) = load.badge_icons.get(&b.id).cloned() {
                            let id = b.id.to_string();
                            imgs.load(&url, move |ui, img| {
                                set_item_thumb(&ui.get_badges(), i, &id, img)
                            });
                        }
                    }
                    for (i, pass) in load.passes.iter().enumerate() {
                        if let Some(url) = load.pass_icons.get(&pass.id).cloned() {
                            let id = pass.id.to_string();
                            imgs.load(&url, move |ui, img| {
                                set_item_thumb(&ui.get_passes(), i, &id, img)
                            });
                        }
                    }

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

                    fetch_servers(&ui, &app2, &bridge2, &imgs, ui.get_server_sort());
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
    badge_icons: std::collections::HashMap<i64, String>,
    pass_icons: std::collections::HashMap<i64, String>,
}

fn fetch_servers(
    ui: &MainWindow,
    app: &Arc<App>,
    bridge: &Arc<Bridge>,
    imgs: &Images,
    sort: i32,
) {
    let imgs2 = imgs.clone();
    let page = app.config.lock().unwrap().server_page_size();
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
        move || async move {
            let list = games::servers(&client, place_id, page, sort).await?;

            let tokens: Vec<String> = list
                .iter()
                .flat_map(|s| {
                    let mut four: Vec<String> = s
                        .player_tokens
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .take(4)
                        .collect();
                    four.resize(4, String::new());
                    four
                })
                .collect();

            let urls = if tokens.iter().any(|t| !t.is_empty()) {
                thumbnails::by_tokens(&client, &tokens).await
            } else {
                vec![None; tokens.len()]
            };

            Ok((list, urls))
        },
        move |ui, result| {
            ui.set_servers_loading(false);
            match result {
                Ok((list, urls)) => {
                    ui.set_servers(ad::model(ad::servers(&list)));

                    for (i, _) in list.iter().enumerate() {
                        for slot in 0..4usize {
                            let Some(Some(url)) = urls.get(i * 4 + slot).cloned() else { continue };
                            imgs2.load(&url, move |ui, img| {
                                set_server_avatar(&ui.get_servers(), i, slot, img)
                            });
                        }
                    }
                }
                Err(e) => {
                    ui.set_servers(ad::model(Vec::new()));
                    bridge::report(&ui, e);
                }
            }
        },
    );
}

fn set_server_avatar(
    model: &slint::ModelRc<ServerRow>,
    index: usize,
    slot: usize,
    img: slint::Image,
) {
    use slint::Model;
    let Some(mut row) = model.row_data(index) else { return };
    match slot {
        0 => row.p0 = img,
        1 => row.p1 = img,
        2 => row.p2 = img,
        _ => row.p3 = img,
    }
    model.set_row_data(index, row);
}

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
    render_library(ui, app);

    match rojoin_launcher::detect() {
        rojoin_launcher::Backend::Sober => {
            if let Some(id) = app.config.lock().unwrap().active_account.clone() {
                match secrets::load(&id) {
                    Some(cookie) => match rojoin_launcher::sober::set_cookie(&cookie) {
                        Ok(true) => {
                            tracing::info!(account = %id, "switched Sober's account");
                            // The client is now on our account, so the warning
                            // that said otherwise is stale.
                            ui.set_account_mismatch(false);
                        }
                        Ok(false) => {
                            tracing::debug!("Sober is already on this account");
                            ui.set_account_mismatch(false);
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "could not switch account before launch");
                            ui.set_launch_error(format!("{e}").into());
                            ui.set_launching(false);
                            return;
                        }
                    },
                    None => tracing::warn!(account = %id, "no stored cookie; launching as whoever Sober has"),
                }
            }

            // Sober holds one global flag set, so the active account's copy has
            // to be written in before the client starts or the previous
            // account's flags would be used.
            let flags: Vec<(String, String)> =
                app.config.lock().unwrap().fflags().into_iter().collect();
            if let Err(e) = rojoin_launcher::sober::write_fflags(&flags) {
                tracing::warn!(error = %e, "could not apply this account's flags");
            }

            match rojoin_launcher::launch_sober(&req) {
                Ok(()) => tracing::info!(
                    place = req.place_id,
                    sub_place = req.is_sub_place(),
                    "launched"
                ),
                Err(e) => {
                    tracing::error!(error = %e, "launch failed");
                    ui.set_launch_error(format!("{e}").into());
                }
            }
            ui.set_launching(false);
        }

        rojoin_launcher::Backend::WindowsClient => {
            let Some(bridge) = app.bridge.lock().unwrap().clone() else {
                tracing::error!("no bridge available to mint a ticket");
                ui.set_launching(false);
                return;
            };

            let client = app.client.clone();
            let req2 = req.clone();

            bridge.call_res(
                move || async move { auth::authentication_ticket(&client).await },
                move |ui, result| {
                    ui.set_launching(false);
                    match result {
                        Ok(ticket) => {
                            let now = chrono::Utc::now().timestamp_millis();
                            match rojoin_launcher::launch_windows(&req2, &ticket, now) {
                                Ok(()) => tracing::info!(
                                    place = req2.place_id,
                                    sub_place = req2.is_sub_place(),
                                    "launched"
                                ),
                                Err(e) => tracing::error!(error = %e, "launch failed"),
                            }
                        }
                        Err(e) => bridge::report(&ui, e),
                    }
                },
            );
        }
    }
}

/// Everything Home needs, in `Send`-safe form. No `slint::Image` here —
/// images are not `Send`, so the tile structs are assembled on the UI thread.
#[derive(Default)]
struct HomeLoad {
    details: Vec<rojoin_roblox::models::GameDetail>,
    votes: Vec<rojoin_roblox::models::Votes>,
    art: std::collections::HashMap<i64, String>,
}

/// Load games the caller already knows by *place* id.
///
/// Local pins are stored as place ids, so this path still has to resolve each
/// one to its universe before the batch endpoints can be used. The Continue
/// sort hands back universe ids directly, which is why `fetch_home` no longer
/// needs any of this.
async fn fetch_by_places(client: &Client, place_ids: &[i64]) -> rojoin_roblox::Result<HomeLoad> {
    let mut universes = Vec::new();
    for place in place_ids {
        if let Ok(u) = games::universe_of(client, *place).await {
            universes.push(u);
        }
    }

    if universes.is_empty() {
        return Ok(HomeLoad::default());
    }

    Ok(HomeLoad {
        details: games::details(client, &universes).await?,
        votes: or_default("ratings", games::votes(client, &universes).await),
        art: or_default("game art", thumbnails::game_art(client, &universes).await),
    })
}

/// Recency comes from Roblox's own Continue sort rather than from anything
/// RoJoin recorded, so a game played on a phone, from the website or through
/// the official client shows up here too.
///
/// It also hands back universe ids directly, which retires the twelve
/// sequential place-to-universe lookups the local history needed.
async fn fetch_home(client: &Client) -> rojoin_roblox::Result<HomeLoad> {
    let universes = discovery::recently_played(client, 12).await?;
    if universes.is_empty() {
        return Ok(HomeLoad::default());
    }

    let mut details = games::details(client, &universes).await?;

    // `details` comes back in Roblox's order, not the order asked for, and
    // recency order is the entire point of this list.
    details.sort_by_key(|d| {
        universes.iter().position(|u| *u == d.id).unwrap_or(usize::MAX)
    });

    Ok(HomeLoad {
        details,
        votes: or_default("ratings", games::votes(client, &universes).await),
        art: or_default("game art", thumbnails::game_art(client, &universes).await),
    })
}

/// Apply a decoded image to one row of a tile model.
///
/// Always called from a deferred context (see `images::Images::load`), never
/// synchronously from a repeater delegate — that path panics with a RefCell
/// double borrow.
fn set_tile_thumb(
    model: &slint::ModelRc<GameTile>,
    index: usize,
    expect_id: &str,
    img: slint::Image,
) {
    use slint::Model;
    let Some(mut row) = model.row_data(index) else { return };
    // The index was captured before the download started. If the model has
    // been replaced since — a second search, a different profile — the row now
    // at that index belongs to something else, and writing to it would show
    // the wrong picture rather than none.
    if row.id != expect_id {
        return;
    }
    row.thumb = img;
    model.set_row_data(index, row);
}

fn set_item_thumb(
    model: &slint::ModelRc<DetailItem>,
    index: usize,
    expect_id: &str,
    img: slint::Image,
) {
    use slint::Model;
    let Some(mut row) = model.row_data(index) else { return };
    // The index was captured before the download started. If the model has
    // been replaced since — a second search, a different profile — the row now
    // at that index belongs to something else, and writing to it would show
    // the wrong picture rather than none.
    if row.id != expect_id {
        return;
    }
    row.thumb = img;
    model.set_row_data(index, row);
}

fn set_wear_thumb(
    model: &slint::ModelRc<WearItem>,
    index: usize,
    expect_id: &str,
    img: slint::Image,
) {
    use slint::Model;
    let Some(mut row) = model.row_data(index) else { return };
    // The index was captured before the download started. If the model has
    // been replaced since — a second search, a different profile — the row now
    // at that index belongs to something else, and writing to it would show
    // the wrong picture rather than none.
    if row.id != expect_id {
        return;
    }
    row.thumb = img;
    model.set_row_data(index, row);
}

fn set_group_icon(model: &slint::ModelRc<GroupRow>, index: usize, img: slint::Image) {
    use slint::Model;
    if let Some(mut row) = model.row_data(index) {
        row.icon = img;
        model.set_row_data(index, row);
    }
}

fn set_friend_avatar(model: &slint::ModelRc<FriendRow>, index: usize, img: slint::Image) {
    use slint::Model;
    if let Some(mut row) = model.row_data(index) {
        if row.is_header {
            return;
        }
        row.avatar = img;
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
