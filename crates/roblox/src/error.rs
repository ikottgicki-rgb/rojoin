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

    /// Roblox wants a captcha (Arkose/FunCaptcha) solved before it will accept
    /// this write. There is no headless answer to this — the action has to be
    /// completed in a browser — so it is a distinct state, not a generic error.
    #[error("{0}")]
    Challenge(String),

    #[error("roblox: {0}")]
    Api(String),
}
