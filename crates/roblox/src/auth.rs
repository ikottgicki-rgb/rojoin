//! Sign-in via Roblox's cross-device quick-login ("log in on another device").
//!
//! Why this and not an embedded browser: it is a *real* browser login — the
//! user approves in their own signed-in browser or the Roblox mobile app — but
//! it needs no webview in our process. That keeps the binary a single small
//! .exe and avoids WebKitGTK, which has hard-crashed on this NVIDIA/Wayland
//! box before.
//!
//! Flow:
//!   1. POST login/create        -> { code, privateKey, expirationTime, imagePath }
//!   2. user confirms the code at roblox.com/crossdevicelogin/ConfirmCode
//!   3. poll login/status        -> Created | Validated | Cancelled
//!   4. POST auth/v2/login       -> Set-Cookie: .ROBLOSECURITY
//!
//! Step 1 is verified working (HTTP 200 with a live code). Step 3 requires the
//! CSRF token — without it Roblox answers 403 "XSRF token invalid". Steps 3-4
//! need a real human approval to exercise, so they are covered by the live
//! sign-in test rather than by anything automated.

use std::time::Duration;

use serde::Deserialize;

use crate::{Error, Result};

const CREATE_URL: &str = "https://apis.roblox.com/auth-token-service/v1/login/create";
const STATUS_URL: &str = "https://apis.roblox.com/auth-token-service/v1/login/status";
const LOGIN_URL: &str = "https://auth.roblox.com/v2/login";
const CONFIRM_BASE: &str = "https://www.roblox.com/crossdevicelogin/ConfirmCode";

/// How often to ask Roblox whether the user has approved yet. Roblox rate
/// limits this endpoint, so do not tighten it without testing.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginCode {
    pub code: String,
    pub private_key: String,
    pub expiration_time: String,
    /// Path to Roblox's own QR image, relative to the auth-token-service host.
    pub image_path: String,
}

impl LoginCode {
    /// The page the user opens to approve. Opened in their real default
    /// browser, never in-process.
    pub fn confirm_url(&self) -> String {
        format!("{CONFIRM_BASE}?code={}", self.code)
    }

    pub fn qr_url(&self) -> String {
        format!("https://apis.roblox.com/auth-token-service{}", self.image_path)
    }

    /// Display form: `7FPRR3` reads far better as `7FP RR3` when someone is
    /// copying it onto a phone.
    pub fn display_code(&self) -> String {
        let c = &self.code;
        if c.len() == 6 {
            format!("{} {}", &c[..3], &c[3..])
        } else {
            c.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStatus {
    /// Created but not yet approved — keep polling.
    Pending,
    /// Approved. Carries no data itself; redeem with `redeem`.
    Validated,
    /// User rejected it, or it timed out.
    Cancelled,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    status: String,
    #[serde(default)]
    account_name: Option<String>,
}

/// Result of a completed sign-in.
#[derive(Debug, Clone)]
pub struct Session {
    pub cookie: String,
    pub account_name: Option<String>,
}

/// Start a sign-in. Returns the code to show the user.
pub async fn create(client: &reqwest::Client) -> Result<LoginCode> {
    let resp = client
        .post(CREATE_URL)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(Error::Api(format!("login/create returned {}", resp.status())));
    }

    Ok(resp.json::<LoginCode>().await?)
}

/// Ask whether the user has approved yet.
///
/// Takes and returns the CSRF token: Roblox answers the first call with 403 and
/// an `x-csrf-token` header, and expects it echoed on every subsequent call.
pub async fn poll(
    client: &reqwest::Client,
    code: &LoginCode,
    csrf: &mut Option<String>,
) -> Result<(LoginStatus, Option<String>)> {
    let body = serde_json::json!({
        "code": code.code,
        "privateKey": code.private_key,
    });

    let mut resp = send_with_csrf(client, STATUS_URL, &body, csrf).await?;

    // One retry: the token can rotate mid-session.
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        capture_csrf(&resp, csrf);
        resp = send_with_csrf(client, STATUS_URL, &body, csrf).await?;
    }

    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(Error::RateLimited);
    }
    if !resp.status().is_success() {
        return Err(Error::Api(format!("login/status returned {}", resp.status())));
    }

    let parsed: StatusResponse = resp.json().await?;
    let status = match parsed.status.as_str() {
        "Validated" => LoginStatus::Validated,
        "Cancelled" => LoginStatus::Cancelled,
        _ => LoginStatus::Pending,
    };
    Ok((status, parsed.account_name))
}

/// Exchange an approved code for a `.ROBLOSECURITY` cookie.
pub async fn redeem(
    client: &reqwest::Client,
    code: &LoginCode,
    account_name: Option<String>,
    csrf: &mut Option<String>,
) -> Result<Session> {
    let body = serde_json::json!({
        "ctype": "AuthToken",
        "cvalue": code.code,
        "password": code.private_key,
    });

    let mut resp = send_with_csrf(client, LOGIN_URL, &body, csrf).await?;
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        capture_csrf(&resp, csrf);
        resp = send_with_csrf(client, LOGIN_URL, &body, csrf).await?;
    }

    if !resp.status().is_success() {
        return Err(Error::Api(format!("v2/login returned {}", resp.status())));
    }

    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(extract_roblosecurity)
        .ok_or_else(|| Error::Api("login succeeded but returned no .ROBLOSECURITY".into()))?;

    Ok(Session { cookie, account_name })
}

// --- helpers ---------------------------------------------------------------

async fn send_with_csrf(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
    csrf: &Option<String>,
) -> Result<reqwest::Response> {
    let mut req = client.post(url).json(body);
    if let Some(token) = csrf {
        req = req.header(crate::CSRF_HEADER, token);
    }
    Ok(req.send().await?)
}

fn capture_csrf(resp: &reqwest::Response, csrf: &mut Option<String>) {
    if let Some(token) = resp
        .headers()
        .get(crate::CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        *csrf = Some(token.to_string());
    }
}

fn extract_roblosecurity(header: &str) -> Option<String> {
    let value = header
        .split(';')
        .next()?
        .trim()
        .strip_prefix(".ROBLOSECURITY=")?;
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_roblosecurity_out_of_set_cookie() {
        let h = ".ROBLOSECURITY=ABC123|def; domain=.roblox.com; path=/; HttpOnly";
        assert_eq!(extract_roblosecurity(h).as_deref(), Some("ABC123|def"));
    }

    #[test]
    fn ignores_other_cookies() {
        assert_eq!(extract_roblosecurity("RBXEventTrackerV2=x; path=/"), None);
    }

    #[test]
    fn ignores_empty_roblosecurity() {
        // Roblox sends an empty one to clear the cookie; treating that as a
        // successful login would sign the user into nothing.
        assert_eq!(extract_roblosecurity(".ROBLOSECURITY=; path=/"), None);
    }

    #[test]
    fn splits_six_char_code_for_display() {
        let c = LoginCode {
            code: "7FPRR3".into(),
            private_key: "k".into(),
            expiration_time: "t".into(),
            image_path: "/p".into(),
        };
        assert_eq!(c.display_code(), "7FP RR3");
        assert!(c.confirm_url().ends_with("ConfirmCode?code=7FPRR3"));
    }
}
