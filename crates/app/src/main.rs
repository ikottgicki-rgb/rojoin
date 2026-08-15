// No console window on Windows. This single line is the difference between
// shipping "a .exe" and shipping "a .exe that flashes a black box on launch".
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicI64, Ordering};

use rojoin_launcher::JoinRequest;
use rojoin_roblox::{auth, chat, friends, games, groups, search, thumbnails, users, Client};
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

    /// Signed-in user id, needed for friends and group calls.
    me: Mutex<i64>,
    /// The friends roster, kept so filtering and pin toggles can re-render
    /// without another round trip to Roblox.
    roster: Mutex<Vec<ad::FriendInput>>,
    offline_collapsed: Mutex<bool>,
    /// Conversations as Roblox returned them, so a row index can be mapped
    /// back to a conversation id and its participants.
    conversations: Mutex<Vec<chat::Conversation>>,
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
        me: Mutex::new(0),
        roster: Mutex::new(Vec::new()),
        offline_collapsed: Mutex::new(false),
        conversations: Mutex::new(Vec::new()),
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
    wire_friends(&ui, &app, &bridge, &imgs);
    wire_profile(&ui, &app, &bridge, &imgs);
    wire_chat(&ui, &app, &bridge, &imgs);

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
    *app.me.lock().unwrap() = user_id;

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
    load_friends(ui, app, bridge, imgs);
    load_conversations(ui, app, bridge, imgs);
}

// ---------------------------------------------------------------------------
// Friends
// ---------------------------------------------------------------------------

fn wire_friends(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_refresh_friends(move || load_friends(&weak.unwrap(), &app, &bridge2, &imgs2));
    }

    // Filter, pin and collapse all re-render from the cached roster — no
    // network round trip, so typing in the filter box stays instant.
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

            // Join *their* server, not a random one: presence gives us both the
            // place and the specific game instance.
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

    bridge.call_res(
        move || async move { fetch_friends(&client, me).await },
        move |ui, result| {
            // Drop the result if the account changed while it was in flight,
            // or one account's roster lands under another account's name.
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

            ui.set_requests_list(ad::model(requests));
            ui.set_friend_requests(load.requests.len() as i32);

            for (i, u) in load.requests.iter().enumerate() {
                if let Some(url) = load.avatars.get(&u.id).cloned() {
                    imgs2.load(&url, move |ui, img| {
                        set_item_thumb(&ui.get_requests_list(), i, img)
                    });
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

    // Capture the avatar URL for each rendered row before handing the model
    // over, so image application can address rows by index.
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
    let Ok(user_id) = id.parse::<i64>() else { return };
    let client = app.client.clone();
    let app2 = app.clone();
    let bridge2 = bridge.clone();
    let imgs2 = imgs.clone();

    bridge.call_res(
        move || async move {
            if accept {
                friends::accept(&client, user_id).await
            } else {
                friends::decline(&client, user_id).await
            }
        },
        move |ui, result| {
            match result {
                // Re-fetch rather than mutating locally: accepting changes both
                // the request list and the roster, and the server is the truth.
                Ok(()) => load_friends(&ui, &app2, &bridge2, &imgs2),
                Err(e) => bridge::report(&ui, e),
            }
        },
    );
}

struct FriendsLoad {
    friends: Vec<ad::FriendInput>,
    requests: Vec<rojoin_roblox::models::User>,
    avatars: std::collections::HashMap<i64, String>,
}

async fn fetch_friends(client: &Client, me: i64) -> rojoin_roblox::Result<FriendsLoad> {
    let list = friends::friend_ids(client, me).await?;
    if !list.complete {
        // Deliberately not fatal: showing a partial roster beats showing none.
        // It is logged so a systematically truncated list is diagnosable.
        tracing::warn!(count = list.ids.len(), "friends list may be incomplete");
    }

    let users = users::batch(client, &list.ids).await.unwrap_or_default();
    let presence = friends::presence(client, &list.ids).await.unwrap_or_default();
    let requests = friends::requests(client, 25).await.unwrap_or_default();

    // One batch for friends and pending requesters together.
    let mut avatar_ids = list.ids.clone();
    avatar_ids.extend(requests.iter().map(|u| u.id));
    let avatars = thumbnails::headshots(client, &avatar_ids).await.unwrap_or_default();

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

    Ok(FriendsLoad { friends, requests, avatars })
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
        ui.on_open_settings(move || tracing::info!("settings: milestone 6"));
    }
    ui.on_open_account(|| tracing::info!("account switcher: milestone 6"));
    ui.on_toggle_notify(|| tracing::info!("game notify: milestone 6"));
    ui.on_copy_link(|| tracing::info!("copy link: milestone 6"));
    ui.on_open_browser(|| tracing::info!("open in browser: milestone 6"));
}

// ---------------------------------------------------------------------------
// Profiles and groups
// ---------------------------------------------------------------------------

fn wire_profile(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    // Every entry point into a profile funnels through one loader.
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
            // The creator can be a user or a group; the detail screen knows
            // which because a group creator has no user profile to open.
            let id = ui.get_game().universe_id.to_string();
            tracing::debug!(%id, "open creator");
            if let Ok(uid) = ui.get_game().creator.parse::<i64>() {
                open_profile(&ui, &app, &bridge2, &imgs2, uid);
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

    // --- social writes ---------------------------------------------------
    //
    // Each flips the button optimistically and reverts if Roblox refuses, so
    // the UI never sits in a state the server disagrees with.
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let weak = ui.as_weak();
        ui.on_profile_add_friend(move || {
            let ui = weak.unwrap();
            let Ok(uid) = ui.get_profile().id.parse::<i64>() else { return };
            let client = app.client.clone();
            ui.set_profile_busy(true);
            bridge2.call_res(
                move || async move { friends::send_request(&client, uid).await },
                move |ui, result| {
                    ui.set_profile_busy(false);
                    match result {
                        Ok(()) => tracing::info!(uid, "friend request sent"),
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
        ui.on_profile_follow(move || {
            let ui = weak.unwrap();
            let Ok(uid) = ui.get_profile().id.parse::<i64>() else { return };
            let was = ui.get_profile().is_following;
            let client = app.client.clone();

            let mut p = ui.get_profile();
            p.is_following = !was;
            ui.set_profile(p);

            bridge2.call_res(
                move || async move {
                    if was {
                        friends::unfollow(&client, uid).await
                    } else {
                        friends::follow(&client, uid).await
                    }
                },
                move |ui, result| {
                    if let Err(e) = result {
                        let mut p = ui.get_profile();
                        p.is_following = was;
                        ui.set_profile(p);
                        bridge::report(&ui, e);
                    }
                },
            );
        });
    }

    // --- group writes ------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

fn wire_chat(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_chat_refresh(move || load_conversations(&weak.unwrap(), &app, &bridge2, &imgs2));
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_chat_select(move |index| {
            let ui = weak.unwrap();
            let convs = app.conversations.lock().unwrap();
            let Some(conv) = convs.get(index as usize).cloned() else { return };
            drop(convs);

            ui.set_chat_selected(index);
            ui.set_chat_title(conv.display_title(*app.me.lock().unwrap()).into());
            ui.set_chat_msgs(ad::model(Vec::new()));
            load_messages(&ui, &app, &bridge2, &imgs2, &conv.id);
        });
    }

    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_chat_send(move |text| {
            let ui = weak.unwrap();
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }

            let index = ui.get_chat_selected();
            let conv_id = {
                let convs = app.conversations.lock().unwrap();
                match convs.get(index as usize) {
                    Some(c) => c.id.clone(),
                    None => return,
                }
            };

            // Optimistic append so the message appears the instant you hit
            // enter; reconciled by the refetch once Roblox confirms.
            append_own_message(&ui, &text);
            ui.set_chat_draft("".into());
            ui.set_chat_sending(true);

            let client = app.client.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs3 = imgs2.clone();
            let cid = conv_id.clone();

            bridge2.call_res(
                move || async move { chat::send(&client, &cid, &text).await },
                move |ui, result| {
                    ui.set_chat_sending(false);
                    match result {
                        Ok(()) => load_messages(&ui, &app2, &bridge3, &imgs3, &conv_id),
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
        ui.on_chat_open_profile(move || {
            let ui = weak.unwrap();
            let index = ui.get_chat_selected();
            let me = *app.me.lock().unwrap();
            let other = app
                .conversations
                .lock()
                .unwrap()
                .get(index as usize)
                .and_then(|c| c.other_participant(me));
            if let Some(uid) = other {
                open_profile(&ui, &app, &bridge2, &imgs2, uid);
            }
        });
    }

    // "Message" on a profile opens (or creates) the one-to-one conversation.
    {
        let app = app.clone();
        let bridge2 = bridge.clone();
        let imgs2 = imgs.clone();
        let weak = ui.as_weak();
        ui.on_profile_message(move || {
            let ui = weak.unwrap();
            let Ok(uid) = ui.get_profile().id.parse::<i64>() else { return };
            let client = app.client.clone();
            let app2 = app.clone();
            let bridge3 = bridge2.clone();
            let imgs3 = imgs2.clone();

            bridge2.call_res(
                move || async move { chat::create_with(&client, uid).await },
                move |ui, result| match result {
                    Ok(_id) => {
                        ui.set_view_kind(0);
                        ui.set_can_back(false);
                        ui.set_section(3);
                        load_conversations(&ui, &app2, &bridge3, &imgs3);
                    }
                    Err(e) => bridge::report(&ui, e),
                },
            );
        });
    }
}

fn load_conversations(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images) {
    ui.set_chat_convs_loading(true);

    let client = app.client.clone();
    let app2 = app.clone();
    let imgs2 = imgs.clone();
    let me = *app.me.lock().unwrap();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move {
            let convs = chat::conversations(&client, 30).await?;
            let ids: Vec<i64> = convs.iter().filter_map(|c| c.other_participant(me)).collect();
            let avatars = thumbnails::headshots(&client, &ids).await.unwrap_or_default();
            Ok((convs, avatars))
        },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_chat_convs_loading(false);

            let (convs, avatars) = match result {
                Ok(v) => v,
                Err(e) => return bridge::report(&ui, e),
            };

            let rows: Vec<ChatConv> = convs
                .iter()
                .map(|c| ChatConv {
                    id: c.id.clone().into(),
                    title: c.display_title(me).into(),
                    preview: c.preview().into(),
                    time: c
                        .last_updated
                        .as_deref()
                        .map(ad::time_ago)
                        .unwrap_or_default()
                        .into(),
                    unread: c.unread_message_count.min(i32::MAX as i64) as i32,
                    online: false,
                    avatar: slint::Image::default(),
                })
                .collect();

            ui.set_chat_convs(ad::model(rows));

            for (i, c) in convs.iter().enumerate() {
                let Some(other) = c.other_participant(me) else { continue };
                let Some(url) = avatars.get(&other).cloned() else { continue };
                imgs2.load(&url, move |ui, img| {
                    set_conv_avatar(&ui.get_chat_convs(), i, img)
                });
            }

            *app2.conversations.lock().unwrap() = convs;
        },
    );
}

fn load_messages(
    ui: &MainWindow,
    app: &Arc<App>,
    bridge: &Arc<Bridge>,
    imgs: &Images,
    conversation_id: &str,
) {
    ui.set_chat_msgs_loading(true);

    let client = app.client.clone();
    let imgs2 = imgs.clone();
    let me = *app.me.lock().unwrap();
    let cid = conversation_id.to_string();
    let cid_for_read = cid.clone();
    let gen = SESSION_GEN.load(Ordering::SeqCst);

    bridge.call_res(
        move || async move {
            let msgs = chat::messages(&client, &cid, 60).await?;
            let ids: Vec<i64> = msgs.iter().map(|m| m.sender_id).collect();
            let avatars = thumbnails::headshots(&client, &ids).await.unwrap_or_default();
            // Opening a conversation is what marks it read.
            let _ = chat::mark_read(&client, &cid).await;
            Ok((msgs, avatars))
        },
        move |ui, result| {
            if gen != SESSION_GEN.load(Ordering::SeqCst) {
                return;
            }
            ui.set_chat_msgs_loading(false);

            let (msgs, avatars) = match result {
                Ok(v) => v,
                Err(e) => return bridge::report(&ui, e),
            };

            // Roblox returns newest first; conversations read oldest at the top.
            let ordered: Vec<_> = msgs.into_iter().rev().collect();

            let mut rows: Vec<ChatMsg> = Vec::with_capacity(ordered.len());
            let mut last_sender = i64::MIN;
            for m in &ordered {
                let mine = m.sender_id == me;
                rows.push(ChatMsg {
                    id: m.id.clone().into(),
                    body: m.content.clone().into(),
                    time: ad::time_ago(&m.created).into(),
                    mine,
                    sender: if mine { "You".into() } else { slint::SharedString::default() },
                    avatar: slint::Image::default(),
                    // Only the first message of a run carries a header, so a
                    // back-and-forth does not repeat the name on every line.
                    show_header: m.sender_id != last_sender,
                });
                last_sender = m.sender_id;
            }
            ui.set_chat_msgs(ad::model(rows));

            for (i, m) in ordered.iter().enumerate() {
                if m.sender_id == me {
                    continue;
                }
                if let Some(url) = avatars.get(&m.sender_id).cloned() {
                    imgs2.load(&url, move |ui, img| {
                        set_msg_avatar(&ui.get_chat_msgs(), i, img)
                    });
                }
            }

            // Clear the unread badge on the row we just opened.
            let idx = ui.get_chat_selected();
            let model = ui.get_chat_convs();
            {
                use slint::Model;
                if idx >= 0 {
                    if let Some(mut row) = model.row_data(idx as usize) {
                        row.unread = 0;
                        model.set_row_data(idx as usize, row);
                    }
                }
            }
            let _ = cid_for_read;
        },
    );
}

/// Optimistic local echo, so a sent message shows immediately.
fn append_own_message(ui: &MainWindow, text: &str) {
    use slint::Model;
    let model = ui.get_chat_msgs();
    let last_mine = model
        .row_data(model.row_count().saturating_sub(1))
        .map(|m| m.mine)
        .unwrap_or(false);

    let mut rows: Vec<ChatMsg> = model.iter().collect();
    rows.push(ChatMsg {
        id: format!("local-{}", rows.len()).into(),
        body: text.into(),
        time: "now".into(),
        mine: true,
        sender: "You".into(),
        avatar: slint::Image::default(),
        show_header: !last_mine,
    });
    ui.set_chat_msgs(ad::model(rows));
}

fn push_view(ui: &MainWindow, app: &Arc<App>, kind: i32) {
    if ui.get_view_kind() == 0 {
        *app.return_section.lock().unwrap() = ui.get_section();
    }
    ui.set_view_kind(kind);
    ui.set_can_back(true);
}

fn open_profile(ui: &MainWindow, app: &Arc<App>, bridge: &Arc<Bridge>, imgs: &Images, user_id: i64) {
    push_view(ui, app, 2);
    ui.set_profile_loading(true);
    ui.set_profile_tab(0);
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
                is_following: false,
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
                    imgs2.load(&url, move |ui, img| {
                        set_tile_thumb(&ui.get_profile_favorites(), i, img)
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
    // Only the user lookup is allowed to fail the whole screen; the rest
    // degrade to empty so one dead endpoint cannot blank a profile.
    let user = users::get(client, user_id).await?;
    let previous_names = users::previous_usernames(client, user_id).await.unwrap_or_default();
    let counts = friends::counts(client, user_id).await;
    let presence = friends::presence(client, &[user_id])
        .await
        .ok()
        .and_then(|v| v.into_iter().next());
    let groups = groups::of_user(client, user_id).await.unwrap_or_default();
    let favorites = games::user_favorites(client, user_id, 12).await.unwrap_or_default();

    let avatar = thumbnails::avatars(client, &[user_id])
        .await
        .ok()
        .and_then(|m| m.get(&user_id).cloned());

    let group_ids: Vec<i64> = groups.iter().map(|m| m.group.id).collect();
    let group_icons = thumbnails::group_icons(client, &group_ids).await.unwrap_or_default();

    let universe_ids: Vec<i64> = favorites.iter().map(|d| d.id).collect();
    let game_art = thumbnails::game_art(client, &universe_ids).await.unwrap_or_default();

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
            let mine = groups::of_user(&client, me).await.unwrap_or_default();
            let membership = mine.into_iter().find(|m| m.group.id == group_id);
            let games_list = games::group_games(&client, group_id, 12).await.unwrap_or_default();

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
            let art = thumbnails::game_art(&client, &universe_ids).await.unwrap_or_default();

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
                    imgs2.load(&url, move |ui, img| {
                        set_tile_thumb(&ui.get_group_games(), i, img)
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

fn set_item_thumb(model: &slint::ModelRc<DetailItem>, index: usize, img: slint::Image) {
    use slint::Model;
    if let Some(mut row) = model.row_data(index) {
        row.thumb = img;
        model.set_row_data(index, row);
    }
}

fn set_conv_avatar(model: &slint::ModelRc<ChatConv>, index: usize, img: slint::Image) {
    use slint::Model;
    if let Some(mut row) = model.row_data(index) {
        row.avatar = img;
        model.set_row_data(index, row);
    }
}

fn set_msg_avatar(model: &slint::ModelRc<ChatMsg>, index: usize, img: slint::Image) {
    use slint::Model;
    if let Some(mut row) = model.row_data(index) {
        row.avatar = img;
        model.set_row_data(index, row);
    }
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
        // The model may have been rebuilt (filter typed, group collapsed)
        // between the request and the reply, so a header now sitting at this
        // index means the image belongs to a list that no longer exists.
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
