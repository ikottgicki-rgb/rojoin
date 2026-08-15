//! The config schema.
//!
//! Structural rule: anything that belongs to a *user* lives under
//! `account_data[user_id]`, never at the top level. v1 kept history, pins and
//! searches globally and they leaked between accounts; unpicking that took a
//! dedicated audit. Starting scoped costs nothing and removes the whole class.
//!
//! Cookies are never in this file. They live in the OS keyring, keyed by
//! account id.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{config_dir, read_json, write_atomic, Result};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// Roblox user id of the active account, as a string. Roblox ids exceed
    /// i32 and JSON numbers are a lossy place to keep them.
    pub active_account: Option<String>,
    pub accounts: Vec<Account>,
    pub account_data: HashMap<String, AccountData>,
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            active_account: None,
            accounts: Vec::new(),
            account_data: HashMap::new(),
            settings: Settings::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountData {
    /// place id -> history
    pub history: HashMap<String, GameHistory>,
    /// Local favourites, deliberately separate from Roblox account favourites.
    pub pins: Vec<String>,
    pub recent_searches: Vec<String>,
    /// Friends lifted to the top of the list. Per-account on purpose: v1 kept
    /// these globally and one account's pins corrupted the other's.
    pub pinned_friends: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GameHistory {
    pub place_id: String,
    pub name: String,
    /// Unix seconds.
    pub last_played: i64,
    pub launches: u32,
    pub playtime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    /// Notifications are opt-in per friend and per game. There is no
    /// "notify me about everything" mode by design.
    pub notify_friends: Vec<String>,
    pub notify_games: Vec<String>,
    pub notify_friend_requests: bool,
    pub close_to_tray: bool,
    /// place id -> account id, so a game can always launch as a chosen alt.
    /// Keyed on the ROOT place id so sub-place launches honour it too.
    pub game_account_bindings: HashMap<String, String>,
    /// User macros. Empty means "use the bundled presets".
    pub macros: Vec<rojoin_macro::Macro>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            notify_friends: Vec::new(),
            notify_games: Vec::new(),
            notify_friend_requests: false,
            close_to_tray: false,
            game_account_bindings: HashMap::new(),
            macros: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> std::path::PathBuf {
        config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let mut cfg: Config = read_json(&Self::path());
        if cfg.version == 0 {
            cfg.version = SCHEMA_VERSION;
        }
        cfg
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&Self::path(), self)
    }

    /// The active account's bucket, created on demand.
    pub fn data_mut(&mut self) -> &mut AccountData {
        let id = self.active_account.clone().unwrap_or_default();
        self.account_data.entry(id).or_default()
    }

    pub fn data(&self) -> Option<&AccountData> {
        self.account_data.get(self.active_account.as_deref()?)
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// Adds or updates an account, and makes it active if it is the first one.
    pub fn upsert_account(&mut self, account: Account) {
        if self.active_account.is_none() {
            self.active_account = Some(account.id.clone());
        }
        match self.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(existing) => *existing = account,
            None => self.accounts.push(account),
        }
    }

    /// Removes an account and everything scoped to it. Returns the id that is
    /// active afterwards, if any.
    pub fn remove_account(&mut self, id: &str) -> Option<String> {
        self.accounts.retain(|a| a.id != id);
        self.account_data.remove(id);
        self.settings.game_account_bindings.retain(|_, v| v != id);

        if self.active_account.as_deref() == Some(id) {
            self.active_account = self.accounts.first().map(|a| a.id.clone());
        }
        self.active_account.clone()
    }

    pub fn record_launch(&mut self, place_id: &str, name: &str, now: i64) {
        let entry = self
            .data_mut()
            .history
            .entry(place_id.to_string())
            .or_default();
        entry.place_id = place_id.to_string();
        if !name.is_empty() {
            entry.name = name.to_string();
        }
        entry.last_played = now;
        entry.launches += 1;
    }

    /// Credited to a named account rather than "whoever is active", so a
    /// mid-session account switch cannot misattribute a play session.
    pub fn add_playtime(&mut self, account_id: &str, place_id: &str, secs: u64) {
        let entry = self
            .account_data
            .entry(account_id.to_string())
            .or_default()
            .history
            .entry(place_id.to_string())
            .or_default();
        entry.place_id = place_id.to_string();
        entry.playtime_secs += secs;
    }

    pub fn push_recent_search(&mut self, query: &str) {
        if query.trim().is_empty() {
            return;
        }
        let data = self.data_mut();
        data.recent_searches.retain(|q| q != query);
        data.recent_searches.insert(0, query.to_string());
        data.recent_searches.truncate(8);
    }

    pub fn toggle_pin(&mut self, place_id: &str) -> bool {
        let data = self.data_mut();
        if let Some(pos) = data.pins.iter().position(|p| p == place_id) {
            data.pins.remove(pos);
            false
        } else {
            data.pins.push(place_id.to_string());
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(id: &str) -> Account {
        Account {
            id: id.into(),
            username: format!("user{id}"),
            display_name: format!("User {id}"),
            avatar_url: String::new(),
        }
    }

    #[test]
    fn first_account_becomes_active() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        assert_eq!(c.active_account.as_deref(), Some("1"));

        c.upsert_account(acct("2"));
        assert_eq!(c.active_account.as_deref(), Some("1"), "second must not steal active");
        assert_eq!(c.accounts.len(), 2);
    }

    #[test]
    fn upsert_updates_rather_than_duplicates() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        let mut updated = acct("1");
        updated.display_name = "Renamed".into();
        c.upsert_account(updated);

        assert_eq!(c.accounts.len(), 1);
        assert_eq!(c.account("1").unwrap().display_name, "Renamed");
    }

    #[test]
    fn history_and_pins_are_scoped_per_account() {
        // The exact leak v1 had: two accounts must not see each other's data.
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        c.upsert_account(acct("2"));

        c.record_launch("606849621", "Jailbreak", 100);
        c.toggle_pin("606849621");

        c.active_account = Some("2".into());
        assert!(c.data().map(|d| d.history.is_empty()).unwrap_or(true));
        assert!(c.data().map(|d| d.pins.is_empty()).unwrap_or(true));

        c.active_account = Some("1".into());
        assert_eq!(c.data().unwrap().history.len(), 1);
        assert_eq!(c.data().unwrap().pins, vec!["606849621".to_string()]);
    }

    #[test]
    fn playtime_credits_the_named_account_not_the_active_one() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        c.upsert_account(acct("2"));

        // Launched as 1, then the user switched to 2 mid-session.
        c.active_account = Some("2".into());
        c.add_playtime("1", "606849621", 300);

        assert_eq!(c.account_data["1"].history["606849621"].playtime_secs, 300);
        assert!(c.account_data.get("2").map(|d| d.history.is_empty()).unwrap_or(true));
    }

    #[test]
    fn removing_an_account_clears_its_data_and_bindings() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        c.upsert_account(acct("2"));
        c.record_launch("123", "Game", 1);
        c.settings
            .game_account_bindings
            .insert("999".into(), "1".into());

        let next = c.remove_account("1");
        assert_eq!(next.as_deref(), Some("2"));
        assert!(!c.account_data.contains_key("1"));
        assert!(c.settings.game_account_bindings.is_empty(), "stale binding left behind");
    }

    #[test]
    fn recent_searches_dedupe_and_cap() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        for i in 0..12 {
            c.push_recent_search(&format!("q{i}"));
        }
        c.push_recent_search("q11");

        let r = &c.data().unwrap().recent_searches;
        assert_eq!(r.len(), 8);
        assert_eq!(r[0], "q11", "re-searching must move it to the front");
        assert_eq!(r.iter().filter(|q| *q == "q11").count(), 1);
    }

    #[test]
    fn blank_search_is_not_recorded() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        c.push_recent_search("   ");

        // Returning early means the bucket is never even created, which is the
        // stronger guarantee: a blank query leaves no trace at all.
        let recorded = c.data().map(|d| d.recent_searches.len()).unwrap_or(0);
        assert_eq!(recorded, 0);
    }
}
