//! The HTTP client every Roblox call goes through.
//!
//! Everything Roblox-specific and annoying lives here so the endpoint modules
//! stay boring:
//!   * the `.ROBLOSECURITY` cookie header,
//!   * the CSRF dance (403 + `x-csrf-token`, retry once),
//!   * backoff on 429 and 5xx,
//!   * mapping a 401 to `Error::Expired` so the UI can raise a sign-in banner
//!     instead of a toast.
//!
//! Rate limiting is the defining constraint. v2 got the *account* throttled by
//! resolving usernames on demand, which surfaced as friends rendering as
//! "User 12345" and, worse, as friends vanishing when a throttled page was
//! cached as a complete list. Batch, cache, and back off — always.

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::sync::RwLock;

use crate::{Error, Result, CSRF_HEADER, USER_AGENT};

/// Retry schedule for 429/5xx. Deliberately patient — being slow beats being
/// throttled, because a throttle affects the user's whole account, not just us.
const BACKOFF: [Duration; 4] = [
    Duration::from_millis(400),
    Duration::from_millis(1200),
    Duration::from_millis(3000),
    Duration::from_millis(7000),
];

/// A response reduced to its headers, for calls where the payload is a header.
pub struct HeaderResponse {
    pub headers: reqwest::header::HeaderMap,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    inner: Arc<Inner>,
}

struct Inner {
    cookie: RwLock<Option<String>>,
    csrf: RwLock<Option<String>>,
    /// A `.ROBLOSECURITY` Roblox handed back mid-session, waiting to be written
    /// to the keyring.
    ///
    /// Roblox re-issues the cookie from time to time and retires the previous
    /// value. Holding the original forever means the stored login quietly stops
    /// working and the next launch lands on the sign-in screen for no reason the
    /// user can see, so a re-issued cookie is adopted immediately and parked
    /// here for the app to persist.
    refreshed: RwLock<Option<String>>,
}

impl Client {
    pub fn new() -> Result<Self> {
        Self::with_timeout(20)
    }

    pub fn with_timeout(secs: u32) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(secs.clamp(5, 120) as u64))
            .build()?;

        Ok(Self {
            http,
            inner: Arc::new(Inner {
                cookie: RwLock::new(None),
                csrf: RwLock::new(None),
                refreshed: RwLock::new(None),
            }),
        })
    }

    /// Raw reqwest client, for the auth flow (which runs before any cookie
    /// exists) and for image fetches.
    pub fn raw(&self) -> &reqwest::Client {
        &self.http
    }

    pub async fn set_cookie(&self, cookie: Option<String>) {
        *self.inner.cookie.write().await = cookie;
        *self.inner.csrf.write().await = None;
        *self.inner.refreshed.write().await = None;
    }

    /// Hand over a cookie Roblox re-issued, once, so the caller can store it.
    ///
    /// Taking it clears it: the value is already live on this client, and the
    /// only thing left to do with it is write it to the keyring.
    pub async fn take_refreshed_cookie(&self) -> Option<String> {
        self.inner.refreshed.write().await.take()
    }

    /// Adopt a `.ROBLOSECURITY` that came back on a response.
    ///
    /// Roblox only sends one when it has replaced the old value, so this is
    /// both the new cookie and notice that the previous one is on its way out.
    async fn absorb_refreshed_cookie(&self, headers: &reqwest::header::HeaderMap) {
        let fresh = headers
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|line| {
                line.split(';')
                    .next()?
                    .trim()
                    .strip_prefix(".ROBLOSECURITY=")
                    .map(str::to_owned)
            })
            .find(|v| !v.is_empty());

        let Some(fresh) = fresh else { return };

        // Adopt only something that is unmistakably a session cookie. Roblox
        // also sends `.ROBLOSECURITY` to *clear* it — on a sign-out, or on an
        // endpoint that decided this request was anonymous — and adopting one of
        // those would overwrite a perfectly good login with a blank, which is
        // the exact failure this whole mechanism exists to prevent.
        if !fresh.starts_with("_|WARNING:-DO-NOT-SHARE-THIS") || fresh.len() < 200 {
            tracing::debug!(
                len = fresh.len(),
                "ignoring a Set-Cookie that is not a usable session cookie"
            );
            return;
        }

        // Roblox echoes the current cookie on plenty of responses; only a
        // genuinely different value is a rotation.
        if self.inner.cookie.read().await.as_deref() == Some(fresh.as_str()) {
            return;
        }

        tracing::info!("Roblox re-issued the session cookie; adopting it");
        *self.inner.cookie.write().await = Some(fresh.clone());
        *self.inner.refreshed.write().await = Some(fresh);
    }

    pub async fn has_cookie(&self) -> bool {
        self.inner.cookie.read().await.is_some()
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let bytes = self.send(Method::Get, url, None).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn post_json<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<T> {
        let bytes = self.send(Method::Post, url, Some(body.clone())).await?;
        if bytes.is_empty() {
            return Ok(serde_json::from_str("null")?);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// A write with no meaningful response — join group, accept friend, etc.
    pub async fn post_action(&self, url: &str) -> Result<()> {
        self.send(Method::Post, url, Some(serde_json::json!({})))
            .await
            .map(|_| ())
    }

    /// A POST whose *response headers* matter, with extra request headers.
    ///
    /// Exists for the authentication-ticket flow, where the ticket comes back
    /// in a header rather than the body and Roblox requires a `Referer`.
    pub async fn post_with_headers(
        &self,
        url: &str,
        body: &serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<HeaderResponse> {
        for attempt in 0..2 {
            let mut req = self.http.post(url).json(body);

            if let Some(cookie) = self.inner.cookie.read().await.as_ref() {
                req = req.header(reqwest::header::COOKIE, format!(".ROBLOSECURITY={cookie}"));
            }
            if let Some(token) = self.inner.csrf.read().await.as_ref() {
                req = req.header(CSRF_HEADER, token);
            }
            for (k, v) in headers {
                req = req.header(*k, *v);
            }

            let resp = req.send().await?;

            if resp.status() == reqwest::StatusCode::FORBIDDEN && attempt == 0 {
                if let Some(token) = resp
                    .headers()
                    .get(CSRF_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
                {
                    *self.inner.csrf.write().await = Some(token);
                    continue;
                }
            }

            if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err(Error::Expired);
            }
            if !resp.status().is_success() {
                return Err(Error::Api(format!("{} for {url}", resp.status())));
            }

            let headers = resp.headers().clone();
            self.absorb_refreshed_cookie(&headers).await;
            return Ok(HeaderResponse { headers });
        }

        Err(Error::Api(format!("{url} kept refusing the CSRF token")))
    }

    /// Authenticated GET returning the raw body, for inspecting a shape.
    pub async fn fetch_json_raw(&self, url: &str) -> Result<String> {
        let bytes = self.send(Method::Get, url, None).await?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    pub async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Api(format!("{} for {url}", resp.status())));
        }
        Ok(resp.bytes().await?.to_vec())
    }

    async fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Vec<u8>> {
        let mut attempt = 0usize;

        loop {
            let resp = self.build(method, url, body.as_ref()).await?.send().await?;
            let status = resp.status();

            if status == reqwest::StatusCode::FORBIDDEN {
                let fresh = resp
                    .headers()
                    .get(CSRF_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);

                if let Some(token) = fresh {
                    let had_one = self.inner.csrf.read().await.is_some();
                    *self.inner.csrf.write().await = Some(token);
                    if !had_one || attempt == 0 {
                        attempt += 1;
                        continue;
                    }
                }

                // A 403 that is not a CSRF problem is usually a captcha
                // challenge. Reporting a bare "forbidden" here hid the reason
                // group joins were failing, so keep what Roblox actually said.
                let challenged = resp.headers().contains_key("rblx-challenge-id")
                    || resp.headers().contains_key("rblx-challenge-type");
                let challenge_type = resp
                    .headers()
                    .get("rblx-challenge-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("none")
                    .to_owned();
                let text = resp.text().await.unwrap_or_default();

                // Writes that Roblox gates (join group, add friend, follow) all
                // land here. Log the whole picture once, so diagnosing the next
                // one does not need another round of guessing.
                tracing::warn!(
                    url,
                    challenged,
                    challenge_type,
                    body = %text.chars().take(300).collect::<String>(),
                    "403 that was not a CSRF retry"
                );

                return Err(if challenged {
                    Error::Challenge(
                        "Roblox wants to verify this one itself. Open it on the \
                         website with the ↗ button and do it there."
                            .into(),
                    )
                } else {
                    Error::Api(first_message(&text))
                });
            }

            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(Error::Expired);
            }

            if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                if let Some(wait) = BACKOFF.get(attempt) {
                    tracing::warn!(%status, url, attempt, "backing off");
                    tokio::time::sleep(*wait).await;
                    attempt += 1;
                    continue;
                }
                return Err(if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    Error::RateLimited
                } else {
                    Error::Api(format!("{status} for {url}"))
                });
            }

            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(Error::Api(format!("{status}: {}", first_message(&text))));
            }

            self.absorb_refreshed_cookie(&resp.headers().clone()).await;
            return Ok(resp.bytes().await?.to_vec());
        }
    }

    async fn build(
        &self,
        method: Method,
        url: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::RequestBuilder> {
        let mut req = match method {
            Method::Get => self.http.get(url),
            Method::Post => self.http.post(url),
        };

        if let Some(cookie) = self.inner.cookie.read().await.as_ref() {
            req = req.header(reqwest::header::COOKIE, format!(".ROBLOSECURITY={cookie}"));
        }
        if let Some(token) = self.inner.csrf.read().await.as_ref() {
            req = req.header(CSRF_HEADER, token);
        }
        if let Some(b) = body {
            req = req.json(b);
        }

        Ok(req)
    }
}

#[derive(Clone, Copy)]
enum Method {
    Get,
    Post,
}

/// Roblox errors come back as `{"errors":[{"code":0,"message":"..."}]}`.
/// Surfacing that message beats surfacing a status code.
fn first_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("errors")?
                .as_array()?
                .first()?
                .get("message")?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                "no detail".into()
            } else {
                trimmed.chars().take(160).collect()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_roblox_error_message() {
        let body = r#"{"errors":[{"code":9,"message":"The specified user does not exist!"}]}"#;
        assert_eq!(first_message(body), "The specified user does not exist!");
    }

    #[test]
    fn falls_back_to_raw_body() {
        assert_eq!(first_message("upstream exploded"), "upstream exploded");
        assert_eq!(first_message("   "), "no detail");
    }

    #[test]
    fn truncates_a_huge_html_error_page() {
        let body = "x".repeat(5000);
        assert_eq!(first_message(&body).len(), 160);
    }

    fn cookie(tag: &str) -> String {
        format!("_|WARNING:-DO-NOT-SHARE-THIS.--Sharing-this-will-allow-someone-to-log-in-as-you.|_{tag}{}", "A".repeat(300))
    }

    fn set_cookie(values: &[&str]) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        for v in values {
            headers.append(reqwest::header::SET_COOKIE, v.parse().unwrap());
        }
        headers
    }

    #[tokio::test]
    async fn a_reissued_cookie_is_adopted_and_offered_once() {
        let client = Client::new().unwrap();
        client.set_cookie(Some(cookie("old"))).await;

        let fresh = cookie("new");
        client
            .absorb_refreshed_cookie(&set_cookie(&[&format!(
                ".ROBLOSECURITY={fresh}; Domain=.roblox.com; Path=/; HttpOnly"
            )]))
            .await;

        assert_eq!(client.take_refreshed_cookie().await.as_deref(), Some(fresh.as_str()));
        // Taking it clears it: there is nothing left to persist.
        assert!(client.take_refreshed_cookie().await.is_none());
    }

    #[tokio::test]
    async fn an_echo_of_the_current_cookie_is_not_a_rotation() {
        let same = cookie("same");
        let client = Client::new().unwrap();
        client.set_cookie(Some(same.clone())).await;

        client
            .absorb_refreshed_cookie(&set_cookie(&[&format!(".ROBLOSECURITY={same}; Path=/")]))
            .await;

        assert!(client.take_refreshed_cookie().await.is_none());
    }

    #[tokio::test]
    async fn a_clearing_cookie_never_overwrites_a_good_login() {
        // Roblox sends these to sign a session out. Adopting one would persist a
        // blank over a working cookie — the exact failure this guards.
        for clearing in [
            ".ROBLOSECURITY=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
            ".ROBLOSECURITY=deleted; Path=/",
            ".ROBLOSECURITY=short-and-not-a-session; Path=/",
        ] {
            let client = Client::new().unwrap();
            client.set_cookie(Some(cookie("good"))).await;
            client.absorb_refreshed_cookie(&set_cookie(&[clearing])).await;

            assert!(
                client.take_refreshed_cookie().await.is_none(),
                "adopted {clearing}"
            );
        }
    }

    #[tokio::test]
    async fn switching_account_drops_a_pending_refresh() {
        let client = Client::new().unwrap();
        client.set_cookie(Some(cookie("first"))).await;
        client
            .absorb_refreshed_cookie(&set_cookie(&[&format!(
                ".ROBLOSECURITY={}; Path=/",
                cookie("rotated")
            )]))
            .await;

        // A pending refresh belongs to the account that produced it.
        client.set_cookie(Some(cookie("second"))).await;
        assert!(client.take_refreshed_cookie().await.is_none());
    }
}
