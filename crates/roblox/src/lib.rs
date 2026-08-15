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
pub mod chat;
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
