//! Game launch pipeline.
//!
//! Linux   — Sober (the Roblox flatpak) via a `roblox://` deep link.
//! Windows — the official Roblox client via `roblox-player:` with an auth ticket.
//!
//! Sub-place joining is a first-class case, not an afterthought. Many games put
//! their real content in a sub-place, and launching the universe's root place
//! drops you in a lobby you then have to walk out of. `JoinRequest::place_id`
//! is whatever place the user actually chose; `root_place_id` is kept alongside
//! it purely so per-game settings (like "always launch this game as <account>")
//! stay keyed to the game rather than to each individual place.

pub mod sober;
pub mod windows;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("launch failed: {0}")]
    Launch(String),
    #[error("Sober is not installed")]
    SoberMissing,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sober,
    WindowsClient,
}

pub fn detect() -> Backend {
    if cfg!(windows) {
        Backend::WindowsClient
    } else {
        Backend::Sober
    }
}

#[derive(Debug, Clone, Default)]
pub struct JoinRequest {
    /// The place the user actually chose — a sub-place when they picked one.
    pub place_id: i64,
    /// The universe's root place. Only used for keying per-game settings.
    pub root_place_id: i64,
    /// Specific server to join, from the server browser.
    pub job_id: Option<String>,
    /// Private-server access code.
    pub access_code: Option<String>,
}

impl JoinRequest {
    pub fn place(place_id: i64) -> Self {
        Self { place_id, root_place_id: place_id, ..Default::default() }
    }

    /// A sub-place launch: join `place_id`, but attribute settings and history
    /// to `root_place_id`.
    pub fn sub_place(place_id: i64, root_place_id: i64) -> Self {
        Self { place_id, root_place_id, ..Default::default() }
    }

    pub fn server(mut self, job_id: impl Into<String>) -> Self {
        self.job_id = Some(job_id.into());
        self
    }

    pub fn is_sub_place(&self) -> bool {
        self.root_place_id != 0 && self.place_id != self.root_place_id
    }

    /// The `roblox://` deep link Sober consumes.
    pub fn roblox_uri(&self) -> String {
        let mut uri = format!(
            "roblox://experiences/start?placeId={}",
            self.place_id
        );
        if let Some(job) = &self.job_id {
            uri.push_str(&format!("&gameInstanceId={job}"));
        }
        if let Some(code) = &self.access_code {
            uri.push_str(&format!("&accessCode={code}"));
        }
        uri
    }
}

/// Launch on Linux. The caller is responsible for having put the right
/// account's cookie in place first.
pub fn launch_sober(req: &JoinRequest) -> Result<()> {
    if !sober::is_installed() {
        return Err(Error::SoberMissing);
    }
    sober::launch_uri(&req.roblox_uri())
}

/// Launch on Windows with a freshly minted authentication ticket.
pub fn launch_windows(req: &JoinRequest, ticket: &str, launch_time_ms: i64) -> Result<()> {
    let inner = windows::place_launcher_url(
        req.place_id,
        req.job_id.as_deref(),
        req.access_code.as_deref(),
    );
    let uri = windows::launch_uri(ticket, &inner, launch_time_ms, 1);
    windows::open_uri(&uri)
}

/// Is a game currently running? Drives the "close Roblox first" prompt before
/// an account switch, and playtime tracking.
pub fn game_running() -> bool {
    match detect() {
        Backend::Sober => sober::is_running(),
        // The Windows client is detected by process name at the call site;
        // there is no equivalent of `flatpak ps` to lean on.
        Backend::WindowsClient => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_join_builds_a_simple_deep_link() {
        let r = JoinRequest::place(606849621);
        assert_eq!(
            r.roblox_uri(),
            "roblox://experiences/start?placeId=606849621"
        );
        assert!(!r.is_sub_place());
    }

    #[test]
    fn sub_place_launches_the_chosen_place_not_the_root() {
        // The whole point of the feature: joining the sub-place directly rather
        // than being dumped in the root lobby.
        let r = JoinRequest::sub_place(999, 606849621);
        assert!(r.is_sub_place());
        assert!(r.roblox_uri().contains("placeId=999"));
        assert!(
            !r.roblox_uri().contains("606849621"),
            "the root place must not be what we launch"
        );
    }

    #[test]
    fn server_join_carries_the_instance_id() {
        let r = JoinRequest::place(1).server("abc-123");
        let uri = r.roblox_uri();
        assert!(uri.contains("placeId=1"));
        assert!(uri.contains("gameInstanceId=abc-123"));
    }

    #[test]
    fn a_place_equal_to_its_root_is_not_a_sub_place() {
        let r = JoinRequest { place_id: 5, root_place_id: 5, ..Default::default() };
        assert!(!r.is_sub_place());
    }

    #[test]
    fn missing_root_is_not_mistaken_for_a_sub_place() {
        // Guards the default-constructed case, where root_place_id is 0.
        let r = JoinRequest { place_id: 5, root_place_id: 0, ..Default::default() };
        assert!(!r.is_sub_place());
    }
}
