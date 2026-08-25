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

use rojoin_roblox::{friends, games, users, Client};

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
            self.sweep_game_stats().await;
        }
    }

    /// Refresh the statistics of favourite and recently-viewed games.
    ///
    /// This is what stops the history chart being a flat line until you happen
    /// to open a game page. Two batched calls cover the lot — details and votes
    /// both take a list of universe ids — so watching a dozen games costs the
    /// same as watching one.
    ///
    /// The store throttles what it keeps, so polling every minute does not mean
    /// storing every minute: samples land at most every half hour, which bounds
    /// the file while still drawing a usable curve.
    async fn sweep_game_stats(&self) {
        use rojoin_store::gamestats;

        /// Enough to cover someone's favourites without turning the poll into a
        /// crawl of every game they have ever opened.
        const MAX_TRACKED: usize = 12;

        let mut games: Vec<(i64, i64)> = self.app.favorite_games.lock().unwrap().clone();
        {
            let store = self.app.stats.lock().unwrap();
            for entry in gamestats::tracked(&store, MAX_TRACKED) {
                if !games.iter().any(|(p, _)| *p == entry.0) {
                    games.push(entry);
                }
            }
        }
        games.truncate(MAX_TRACKED);
        if games.is_empty() {
            return;
        }

        // Skip the round trip entirely if nothing is due to be stored yet.
        let now = chrono::Utc::now().timestamp();
        let due: Vec<(i64, i64)> = {
            let store = self.app.stats.lock().unwrap();
            games
                .into_iter()
                .filter(|(place, _)| {
                    store
                        .get(&place.to_string())
                        .and_then(|s| s.last())
                        .map(|last| now - last.at >= gamestats::MIN_GAP_SECS)
                        .unwrap_or(true)
                })
                .collect()
        };
        if due.is_empty() {
            return;
        }

        let universes: Vec<i64> = due.iter().map(|(_, u)| *u).collect();
        let details = match games::details(&self.client, &universes).await {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, "could not refresh game statistics");
                return;
            }
        };
        let votes = games::votes(&self.client, &universes).await.unwrap_or_default();

        let keep = match self.app.config.lock() {
            Ok(c) => c.prune_window_days(),
            Err(_) => return,
        };

        let mut store = self.app.stats.lock().unwrap();
        let mut stored = 0usize;

        for d in &details {
            // Key on the root place, matching what the game page records, or the
            // same game would accumulate two separate series.
            let place = if d.root_place_id != 0 { d.root_place_id } else { continue };
            let v = votes.iter().find(|v| v.id == d.id);

            let sample = gamestats::Sample {
                at: now,
                universe_id: d.id,
                playing: d.playing,
                visits: d.visits,
                upvotes: v.map(|v| v.up_votes).unwrap_or(0),
                downvotes: v.map(|v| v.down_votes).unwrap_or(0),
            };

            if gamestats::record(store.entry(place.to_string()).or_default(), sample, keep) {
                stored += 1;
            }
        }

        if stored > 0 {
            if let Err(e) = gamestats::save(&store) {
                tracing::warn!(error = %e, "could not save game statistics");
            } else {
                tracing::info!(games = stored, "recorded game statistics");
            }
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

        // Checked once per sweep rather than per presence row: it is a process
        // lookup, and only your own account is gated on it.
        let client_running = rojoin_launcher::game_running();

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
                    // Presence alone is not proof *you* are playing. Roblox keeps
                    // reporting a stale "in game" after an unclean exit — suspend
                    // the machine mid-session and it can persist for hours, which
                    // invented a ten-minute session on a day nothing was played.
                    // The local client either exists or it does not, so for your
                    // own account that is the authority.
                    if !client_running {
                        tracing::debug!(
                            "presence says in-game but no Roblox process; ignoring"
                        );
                        continue;
                    }
                    cfg.observe_session(universe, root, &p.location, now, gap);
                    // Logged at info, not debug: without this there is no way to
                    // tell whether tracking is working short of waiting for a bar
                    // to appear on the graph.
                    tracing::info!(
                        game = if p.location.is_empty() { "hidden" } else { p.location.as_str() },
                        "playtime: recorded you in a game"
                    );
                } else {
                    cfg.observe_friend_session(
                        &p.user_id.to_string(),
                        universe,
                        root,
                        &p.location,
                        now,
                        gap,
                    );
                    tracing::info!(
                        friend = p.user_id,
                        game = if p.location.is_empty() { "hidden" } else { p.location.as_str() },
                        "playtime: recorded a pinned friend in a game"
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

        if !touched {
            tracing::debug!("playtime: nobody tracked is in a game");
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

/// Windows toast, through the WinRT notification API.
///
/// `notify-rust` is linux/bsd/mac only, so there is nothing to reuse. The
/// alternatives were each worse: a WinRT crate does not cross-compile cleanly to
/// the mingw target this ships from, and a `Shell_NotifyIcon` balloon needs its
/// own tray icon, which would mean either a second icon in the tray or
/// hand-rolling the tray that already works.
///
/// So this drives the same WinRT types through PowerShell, which is present on
/// every supported Windows and needs no registration. It costs a few hundred
/// milliseconds per notification — irrelevant for something that fires when a
/// friend starts a game — and runs with no window, since this build goes to
/// lengths not to show a console.
#[cfg(windows)]
fn show(summary: &str, body: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // Toast content is XML, and a game called "Bob's & Friends <3" would
    // otherwise produce a document that does not parse.
    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    let script = format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, \
         ContentType = WindowsRuntime] > $null; \
         $x = New-Object Windows.Data.Xml.Dom.XmlDocument; \
         $x.LoadXml('<toast><visual><binding template=\"ToastText02\">\
         <text id=\"1\">{}</text><text id=\"2\">{}</text></binding></visual></toast>'); \
         $t = New-Object Windows.UI.Notifications.ToastNotification $x; \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('RoJoin').Show($t)",
        xml_escape(summary),
        xml_escape(body),
    );

    std::thread::spawn(move || {
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match result {
            Ok(s) if s.success() => {}
            Ok(s) => tracing::debug!(code = ?s.code(), "toast command failed"),
            Err(e) => tracing::debug!(error = %e, "could not run the toast command"),
        }
    });
}

#[cfg(not(any(unix, windows)))]
fn show(summary: &str, body: &str) {
    tracing::info!(summary, body, "notification (no backend on this platform)");
}

#[cfg(test)]
mod tests {
    /// The Windows path builds XML by hand, so a name with an ampersand or a
    /// quote in it must not produce a document that fails to parse. Roblox game
    /// names contain both routinely.
    #[cfg(windows)]
    #[test]
    fn toast_text_is_xml_escaped() {
        fn xml_escape(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
                .replace('\'', "&apos;")
        }

        assert_eq!(xml_escape("Bob's & Friends <3"), "Bob&apos;s &amp; Friends &lt;3");
        assert_eq!(xml_escape(r#"say "hi""#), "say &quot;hi&quot;");
        assert_eq!(xml_escape("plain"), "plain");
    }
}
