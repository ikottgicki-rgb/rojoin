//! Roblox web API client.
//!
//! Layout: `client` owns everything Roblox-specific and annoying (cookie, CSRF,
//! backoff, error mapping); every other module is a thin, boring set of
//! endpoint functions on top of it.
//!
//! Endpoint facts worth not re-learning the hard way:
//!   * `chat.roblox.com` is **dead** (404). Chat lives at
//!     `apis.roblox.com/platform-chat-api/v1/*`.
//!   * `games.roblox.com/v1/games/list` search is **dead** (404). Use
//!     `search-api/omni-search`.
//!   * `games/{id}/servers/Public` keys on **placeId**; a universeId gives 400.
//!   * Sub-places come from `develop.roblox.com/v1/universes/{id}/places` and
//!     that endpoint needs no auth.

pub mod auth;
pub mod avatar;
pub mod client;
pub mod error;
pub mod friends;
pub mod games;
pub mod groups;
pub mod models;
pub mod search;
pub mod thumbnails;
pub mod users;

pub use client::Client;
pub use error::{Error, Result};

/// Roblox rejects the first state-changing request with 403 and hands back an
/// `x-csrf-token` header; every write retries once with it.
pub const CSRF_HEADER: &str = "x-csrf-token";

pub const USER_AGENT: &str = concat!("RoJoin/", env!("CARGO_PKG_VERSION"));

/// Round a page size up to one Roblox will accept.
///
/// The paged endpoints take *only* 10, 25, 50 or 100 — anything else is a 400
/// (`Allowed values: 10, 25, 50, 100`), not a clamp. Asking for 12 therefore
/// returns nothing at all, which from the caller's side is indistinguishable
/// from "there are none". Every paged call goes through this.
pub(crate) fn page_limit(requested: u32) -> u32 {
    match requested {
        0..=10 => 10,
        11..=25 => 25,
        26..=50 => 50,
        _ => 100,
    }
}
