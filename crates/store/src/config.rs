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
    /// Friend requests already accepted or declined. Roblox keeps serving a
    /// handled request for a while, so without this the row reappears on the
    /// next refresh. Per-account: a request handled on one account says nothing
    /// about the same person's request to another.
    #[serde(default)]
    pub handled_requests: Vec<String>,
    /// Engine flags for this account.
    ///
    /// Sober keeps one global flag set, so RoJoin owns the per-account copy and
    /// writes the active account's into Sober before a launch. That makes
    /// Sober's config derived state rather than the source of truth.
    #[serde(default)]
    pub fflags: HashMap<String, String>,
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
    /// place id -> account id, so a game can always launch as a chosen alt.
    /// User macros. Empty means "use the bundled presets".
    pub macros: Vec<rojoin_macro::Macro>,

    /// Master switch. When off, no hotkey fires anything.
    pub macros_enabled: bool,
    /// Only run macros while Roblox is the focused window.
    ///
    /// On by default: without it a hotkey fires wherever you are, so F3 would
    /// freeze your game while you are typing in a browser.
    pub macros_only_when_focused: bool,
    /// Key that stops every running macro and releases any freeze.
    pub panic_key: String,

    /// 1.0 = normal. Clamped when applied.
    pub ui_scale: f32,
    /// Section index to open on launch.
    pub startup_section: i32,
    /// Seconds between friend-presence refreshes. Lower is more current and
    /// more likely to get the account rate-limited.
    pub presence_refresh_secs: u32,
    /// Ask before leaving a group or removing an account.
    pub confirm_destructive: bool,
    /// Friend requests already accepted or declined. Roblox keeps returning
    /// them for a while, so they are filtered out of every refetch — otherwise
    /// a declined request reappears and looks like the click did nothing.

    /// Which home sections are shown, in order. Kinds:
    /// 0 status · 1 jump back in · 2 pinned · 3 recent · 4 favourites
    pub home_sections: Vec<i32>,

    /// How many servers to ask for on a game's server list.
    #[serde(default)]
    pub server_page_size: u32,
    /// Seconds before an API call is abandoned.
    #[serde(default)]
    pub request_timeout_secs: u32,
    /// Thumbnails held in memory before the oldest are evicted.
    #[serde(default)]
    pub image_cache_size: u32,
    /// Write debug-level logs, and mirror them to a file in the data folder.
    #[serde(default)]
    pub verbose_logging: bool,
    /// Fetch and stage a new release on startup without being asked. Default
    /// on; the swap still only takes effect on the next launch.
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// Named flag sets, shared across accounts so a tuned set can be reused.
    #[serde(default)]
    pub fflag_presets: HashMap<String, HashMap<String, String>>,
}

/// serde needs a function, not a literal, for a defaulted `true`.
fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            notify_friends: Vec::new(),
            notify_games: Vec::new(),
            notify_friend_requests: false,
            macros: Vec::new(),
            macros_enabled: true,
            macros_only_when_focused: true,
            panic_key: "F8".into(),
            ui_scale: 1.0,
            startup_section: 0,
            presence_refresh_secs: 60,
            confirm_destructive: true,
            home_sections: vec![0, 1, 2, 3, 4],
            // Zero means "unset": the accessor picks the real default, so an
            // older config file keeps working without a migration.
            server_page_size: 0,
            request_timeout_secs: 0,
            image_cache_size: 0,
            verbose_logging: false,
            auto_update: true,
            fflag_presets: HashMap::new(),
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

    /// Remember that a friend request was answered, capped so the list cannot
    /// grow without bound over the life of an account.
    pub fn remember_handled_request(&mut self, key: String) -> bool {
        let data = self.data_mut();
        if data.handled_requests.contains(&key) {
            return false;
        }
        data.handled_requests.push(key);
        let len = data.handled_requests.len();
        if len > 200 {
            data.handled_requests.drain(0..len - 200);
        }
        true
    }

    /// The active account's flags.
    pub fn fflags(&self) -> HashMap<String, String> {
        self.data().map(|d| d.fflags.clone()).unwrap_or_default()
    }

    /// Set or clear one flag on the active account. An empty value removes it.
    pub fn set_fflag(&mut self, name: &str, value: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let data = self.data_mut();
        if value.trim().is_empty() {
            data.fflags.remove(&name);
        } else {
            data.fflags.insert(name, value.trim().to_string());
        }
    }

    /// Replace the active account's flags wholesale, for loading a preset.
    pub fn set_fflags(&mut self, flags: HashMap<String, String>) {
        self.data_mut().fflags = flags;
    }

    pub fn save_preset(&mut self, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let flags = self.fflags();
        self.settings.fflag_presets.insert(name, flags);
    }

    pub fn delete_preset(&mut self, name: &str) {
        self.settings.fflag_presets.remove(name);
    }

    pub fn preset_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.settings.fflag_presets.keys().cloned().collect();
        names.sort();
        names
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

    /// Server page size, snapped to a value Roblox accepts. Zero means unset,
    /// which is the common case for a config written before this existed.
    pub fn server_page_size(&self) -> u32 {
        match self.settings.server_page_size {
            10 | 25 | 50 | 100 => self.settings.server_page_size,
            _ => 25,
        }
    }

    /// Bounded either side: a two-second timeout makes every call fail on a
    /// slow link, and a five-minute one makes the app look frozen.
    pub fn request_timeout_secs(&self) -> u32 {
        match self.settings.request_timeout_secs {
            0 => 20,
            n => n.clamp(5, 120),
        }
    }

    pub fn image_cache_size(&self) -> u32 {
        match self.settings.image_cache_size {
            0 => 400,
            n => n.clamp(50, 2000),
        }
    }

    /// Clamped so a bad config cannot make the app unusable.
    pub fn ui_scale(&self) -> f32 {
        if self.settings.ui_scale.is_finite() {
            self.settings.ui_scale.clamp(0.8, 1.6)
        } else {
            1.0
        }
    }

    /// Clamped so nobody can set a 1-second poll and throttle their account.
    pub fn presence_refresh_secs(&self) -> u32 {
        self.settings.presence_refresh_secs.clamp(30, 600)
    }

    /// Home sections, falling back to the default when the list is empty or
    /// holds only unknown kinds — an empty Home reads as a broken app.
    pub fn home_sections(&self) -> Vec<i32> {
        let valid: Vec<i32> = self
            .settings
            .home_sections
            .iter()
            .copied()
            .filter(|k| (0..=4).contains(k))
            .collect();
        if valid.is_empty() {
            vec![0, 1, 2, 3, 4]
        } else {
            valid
        }
    }

    pub fn toggle_home_section(&mut self, kind: i32) {
        let mut current = self.home_sections();
        match current.iter().position(|k| *k == kind) {
            Some(i) => {
                current.remove(i);
            }
            None => current.push(kind),
        }
        self.settings.home_sections = current;
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

        c.active_account = Some("2".into());
        c.add_playtime("1", "606849621", 300);

        assert_eq!(c.account_data["1"].history["606849621"].playtime_secs, 300);
        assert!(c.account_data.get("2").map(|d| d.history.is_empty()).unwrap_or(true));
    }

    #[test]
    fn removing_an_account_clears_its_data() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        c.upsert_account(acct("2"));
        c.record_launch("123", "Game", 1);

        let next = c.remove_account("1");
        assert_eq!(next.as_deref(), Some("2"));
        assert!(!c.account_data.contains_key("1"));
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
    fn home_never_ends_up_empty() {
        let mut c = Config::default();
        c.settings.home_sections = vec![];
        assert_eq!(c.home_sections().len(), 5, "an empty Home reads as broken");

        c.settings.home_sections = vec![99, -1];
        assert_eq!(c.home_sections().len(), 5, "unknown kinds must not blank it");
    }

    #[test]
    fn home_sections_toggle_off_and_back_on() {
        let mut c = Config::default();
        c.toggle_home_section(3);
        assert!(!c.home_sections().contains(&3));
        c.toggle_home_section(3);
        assert!(c.home_sections().contains(&3));
    }

    #[test]
    fn ui_scale_is_clamped_to_something_usable() {
        let mut c = Config::default();
        assert_eq!(c.ui_scale(), 1.0);

        c.settings.ui_scale = 99.0;
        assert_eq!(c.ui_scale(), 1.6);
        c.settings.ui_scale = 0.01;
        assert_eq!(c.ui_scale(), 0.8);
        c.settings.ui_scale = f32::NAN;
        assert_eq!(c.ui_scale(), 1.0);
    }

    #[test]
    fn presence_refresh_cannot_be_set_fast_enough_to_throttle_the_account() {
        let mut c = Config::default();
        c.settings.presence_refresh_secs = 1;
        assert_eq!(c.presence_refresh_secs(), 30);
        c.settings.presence_refresh_secs = 99_999;
        assert_eq!(c.presence_refresh_secs(), 600);
    }

    #[test]
    fn macro_safety_defaults_are_on() {
        let c = Config::default();
        assert!(c.settings.macros_only_when_focused, "focus gate must default on");
        assert!(c.settings.confirm_destructive);
        assert_eq!(c.settings.panic_key, "F8");
    }

    #[test]
    fn blank_search_is_not_recorded() {
        let mut c = Config::default();
        c.upsert_account(acct("1"));
        c.push_recent_search("   ");

        let recorded = c.data().map(|d| d.recent_searches.len()).unwrap_or(0);
        assert_eq!(recorded, 0);
    }
}

#[cfg(test)]
mod handled_request_tests {
    use super::*;

    #[test]
    fn the_handled_list_stays_capped_however_long_an_account_lives() {
        let mut cfg = Config::default();
        cfg.active_account = Some("1".into());
        for i in 0..500 {
            cfg.remember_handled_request(i.to_string());
        }
        let data = cfg.data().unwrap();
        assert_eq!(data.handled_requests.len(), 200);
        // the newest survive, the oldest are dropped
        assert!(data.handled_requests.contains(&"499".to_string()));
        assert!(!data.handled_requests.contains(&"0".to_string()));
    }

    #[test]
    fn answering_the_same_request_twice_is_not_recorded_twice() {
        let mut cfg = Config::default();
        cfg.active_account = Some("1".into());
        assert!(cfg.remember_handled_request("7".into()));
        assert!(!cfg.remember_handled_request("7".into()));
        assert_eq!(cfg.data().unwrap().handled_requests.len(), 1);
    }
}

#[cfg(test)]
mod fflag_tests {
    use super::*;

    fn with_account(id: &str) -> Config {
        let mut c = Config::default();
        c.upsert_account(Account {
            id: id.into(),
            username: id.into(),
            display_name: id.into(),
            avatar_url: String::new(),
        });
        c
    }

    #[test]
    fn flags_belong_to_one_account_and_do_not_leak_to_another() {
        let mut c = with_account("1");
        c.set_fflag("FFlagOne", "true");
        assert_eq!(c.fflags().get("FFlagOne").map(String::as_str), Some("true"));

        c.upsert_account(Account {
            id: "2".into(),
            username: "2".into(),
            display_name: "2".into(),
            avatar_url: String::new(),
        });
        c.active_account = Some("2".into());
        assert!(c.fflags().is_empty(), "the second account inherited flags");

        c.active_account = Some("1".into());
        assert_eq!(c.fflags().len(), 1, "the first account lost its flags");
    }

    #[test]
    fn an_empty_value_clears_a_flag() {
        let mut c = with_account("1");
        c.set_fflag("FFlagOne", "true");
        c.set_fflag("FFlagOne", "   ");
        assert!(c.fflags().is_empty());
    }

    #[test]
    fn a_nameless_flag_is_refused_rather_than_stored() {
        let mut c = with_account("1");
        c.set_fflag("   ", "true");
        assert!(c.fflags().is_empty());
    }

    #[test]
    fn a_preset_captures_the_current_set_and_survives_changing_it() {
        let mut c = with_account("1");
        c.set_fflag("FFlagOne", "true");
        c.save_preset("fps");

        c.set_fflag("FFlagOne", "false");
        c.set_fflag("FFlagTwo", "1");

        let preset = c.settings.fflag_presets["fps"].clone();
        assert_eq!(preset.len(), 1, "the preset followed later edits");
        assert_eq!(preset["FFlagOne"], "true");

        c.set_fflags(preset);
        assert_eq!(c.fflags().len(), 1);
        assert_eq!(c.fflags()["FFlagOne"], "true");
    }

    #[test]
    fn presets_are_listed_in_a_stable_order() {
        let mut c = with_account("1");
        for n in ["zed", "alpha", "mid"] {
            c.save_preset(n);
        }
        assert_eq!(c.preset_names(), vec!["alpha", "mid", "zed"]);
        c.delete_preset("mid");
        assert_eq!(c.preset_names(), vec!["alpha", "zed"]);
    }
}
