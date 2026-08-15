use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),

    #[error("decode: {0}")]
    Decode(#[from] serde_json::Error),

    /// The cookie is no longer valid. The UI raises a sign-in banner for this
    /// rather than a toast — an expired session is a state, not an incident.
    #[error("session expired")]
    Expired,

    #[error("rate limited")]
    RateLimited,

    #[error("roblox: {0}")]
    Api(String),
}
