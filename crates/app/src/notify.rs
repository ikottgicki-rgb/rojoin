//! Desktop notifications for watched friends and friend requests.
//!
//! Strictly opt-in: nothing fires unless the user has subscribed to that
//! specific friend, or turned friend-request notifications on. There is no
//! "notify me about everything" mode, by design.
//!
//! The watcher polls presence rather than holding a socket open, because
//! Roblox has no push channel we can use and polling at this interval is well
//! inside the rate limits.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rojoin_roblox::{friends, users, Client};

/// Slow on purpose. Presence is not urgent, and a tight loop here is the
/// fastest way to get the whole account throttled.
const POLL: Duration = Duration::from_secs(60);

pub struct Watcher {
    client: Client,
    app: Arc<crate::App>,
}

impl Watcher {
    pub fn new(client: Client, app: Arc<crate::App>) -> Self {
        Self { client, app }
    }

    /// Poll forever. Spawned on the tokio runtime; ends when the app does.
    pub async fn run(self) {
        let mut previous: HashMap<i64, bool> = HashMap::new();
        let mut previous_requests: Option<i64> = None;

        loop {
            tokio::time::sleep(POLL).await;

            self.persist_refreshed_cookie().await;

            let (watched, want_requests) = {
                let cfg = match self.app.config.lock() {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                (
                    cfg.settings
                        .notify_friends
                        .iter()
                        .filter_map(|id| id.parse::<i64>().ok())
                        .collect::<Vec<_>>(),
                    cfg.settings.notify_friend_requests,
                )
            };

            if !watched.is_empty() {
                self.sweep_presence(&watched, &mut previous).await;
            }

            if want_requests {
                self.sweep_requests(&mut previous_requests).await;
            }

            self.sweep_playtime().await;
        }
    }

    /// Record who is playing what, for you and for pinned friends.
    ///
    /// Presence rather than watching for a game process, which buys three
    /// things: it is identical on Windows and Linux with no platform code, it
    /// catches a session started anywhere — the website, a phone, the official
    /// client — and it names the game, which a process cannot.
    ///
    /// The cost is that it samples. A session's length is only known to within
    /// one poll, and nothing is recorded while RoJoin is closed. Both are
    /// honest limits of watching from outside rather than from inside the game.
    async fn sweep_playtime(&self) {
        let (me, pinned, track_friends) = {
            let cfg = match self.app.config.lock() {
                Ok(c) => c,
                Err(_) => return,
            };
            let pinned: Vec<i64> = cfg
                .data()
                .map(|d| {
                    d.pinned_friends
                        .iter()
                        .filter_map(|id| id.parse::<i64>().ok())
                        .collect()
                })
                .unwrap_or_default();
            (
                *self.app.me.lock().unwrap_or_else(|e| e.into_inner()),
                pinned,
                cfg.settings.track_friend_playtime,
            )
        };

        let mut ids = Vec::new();
        if me != 0 {
            ids.push(me);
        }
        if track_friends {
            ids.extend(pinned.iter().copied().filter(|id| *id != me));
        }
        if ids.is_empty() {
            return;
        }

        let presences = match friends::presence(&self.client, &ids).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "playtime sweep could not read presence");
                return;
            }
        };

        // Three polls of slack. One missed sweep — a hiccup, a throttle — should
        // not split a single sitting into two sessions.
        let gap = POLL.as_secs() as i64 * 3;
        let now = chrono::Utc::now().timestamp();
        let mut touched = false;

        {
            let mut cfg = match self.app.config.lock() {
                Ok(c) => c,
                Err(_) => return,
            };

            for p in &presences {
                if p.kind != friends::PresenceKind::InGame {
                    continue;
                }
                // Null when the account's privacy hides where they are. The time
                // is still real, so it is recorded against the unknown bucket
                // rather than dropped.
                let universe = p.universe_id.unwrap_or(rojoin_store::playtime::UNKNOWN_UNIVERSE);
                let root = p.root_place_id.or(p.place_id).unwrap_or(0);

                if p.user_id == me {
                    cfg.observe_session(universe, root, &p.location, now, gap);
                } else {
                    cfg.observe_friend_session(
                        &p.user_id.to_string(),
                        universe,
                        root,
                        &p.location,
                        now,
                        gap,
                    );
                }
                touched = true;
            }

            if touched {
                if let Err(e) = cfg.save() {
                    tracing::warn!(error = %e, "could not persist play sessions");
                }
            }
        }

        if touched {
            tracing::debug!("recorded a playtime sample");
        }
    }

    /// Write through a cookie Roblox re-issued mid-session.
    ///
    /// This loop is the app's one dependable heartbeat, so it is where the
    /// write-through lives. Without it a rotation is only ever held in memory:
    /// the keyring keeps the retired value, and the next launch finds a dead
    /// cookie and asks the user to sign in again for no visible reason.
    async fn persist_refreshed_cookie(&self) {
        let Some(cookie) = self.client.take_refreshed_cookie().await else {
            return;
        };

        let active = match self.app.config.lock() {
            Ok(cfg) => cfg.active_account.clone(),
            Err(_) => return,
        };

        let Some(id) = active else { return };

        match crate::secrets::store(&id, &cookie) {
            Ok(()) => tracing::info!(account = %id, "stored the re-issued cookie"),
            Err(e) => tracing::error!(error = %e, "could not store the re-issued cookie"),
        }
    }

    async fn sweep_presence(&self, watched: &[i64], previous: &mut HashMap<i64, bool>) {
        let Ok(presences) = friends::presence(&self.client, watched).await else {
            return;
        };

        let mut newly_playing = Vec::new();

        for p in &presences {
            let in_game = p.kind == friends::PresenceKind::InGame;
            let was = previous.insert(p.user_id, in_game);
            if in_game && was == Some(false) {
                newly_playing.push(p.clone());
            }
        }

        if newly_playing.is_empty() {
            return;
        }

        let ids: Vec<i64> = newly_playing.iter().map(|p| p.user_id).collect();
        let names = users::batch(&self.client, &ids).await.unwrap_or_default();

        for p in newly_playing {
            let who = names
                .iter()
                .find(|u| u.id == p.user_id)
                .map(|u| u.display_name.clone())
                .unwrap_or_else(|| format!("User {}", p.user_id));

            let body = if p.location.is_empty() {
                format!("{who} is in a game")
            } else {
                format!("{who} is playing {}", p.location)
            };
            show("RoJoin", &body);
        }
    }

    async fn sweep_requests(&self, previous: &mut Option<i64>) {
        let Ok(count) = friends::request_count(&self.client).await else {
            return;
        };

        match *previous {
            None => {}
            Some(before) if count > before => {
                let delta = count - before;
                show(
                    "RoJoin",
                    &if delta == 1 {
                        "You have a new friend request".to_string()
                    } else {
                        format!("You have {delta} new friend requests")
                    },
                );
            }
            _ => {}
        }
        *previous = Some(count);
    }
}

#[cfg(unix)]
fn show(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        if let Err(e) = notify_rust::Notification::new()
            .summary(&summary)
            .body(&body)
            .appname("RoJoin")
            .show()
        {
            tracing::debug!(error = %e, "could not show a notification");
        }
    });
}

#[cfg(not(unix))]
fn show(summary: &str, body: &str) {
    tracing::info!(summary, body, "notification (no Windows backend yet)");
}
