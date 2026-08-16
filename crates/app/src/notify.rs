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
