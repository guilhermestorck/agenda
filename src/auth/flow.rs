//! The installed-application flow: consent in the browser, code on the loopback, tokens
//! from Google, refresh thereafter.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::keyring::Tokens;
use super::{Pkce, Redirect, parse_redirect, random_token};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Overridable so the refresh-and-retry path can be exercised against a local server
/// instead of being asserted about.
pub const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Full calendar read/write, plus the account's own profile.
///
/// `calendar.readonly` would be a truer fit for v1, but the write path is deferred rather
/// than abandoned, and widening a scope later forces every account to re-consent.
/// `userinfo.profile` is here for the same reason: it supplies the name and picture the
/// sidebar shows, and adding it once accounts exist would cost every one of them a
/// re-consent. It grants no access to anyone's data but the signed-in account's own profile.
const SCOPE: &str = "https://www.googleapis.com/auth/calendar \
                     https://www.googleapis.com/auth/userinfo.profile";

/// Google has permanently rejected the refresh token: the account must consent again and no
/// retry helps. Typed because the caller must branch on it (SPEC §7) — this is the
/// difference between "show Reconnect" and "try again in a minute".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshRejected {
    pub account: String,
    pub reason: String,
}

impl std::fmt::Display for RefreshRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Google rejected the stored credentials for {} ({}); the account must be reconnected",
            self.account, self.reason
        )
    }
}

impl std::error::Error for RefreshRejected {}

/// Google's answer at the token endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    /// Absent on a refresh: Google reissues one only when the old one is being replaced.
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: String,
    error_description: Option<String>,
}

/// Where to send the browser. `access_type=offline` with `prompt=consent` is what makes
/// Google issue a refresh token at all; without both, a restart means signing in again.
pub fn authorization_url(
    client_id: &str,
    redirect_uri: &str,
    pkce: &Pkce,
    state: &str,
) -> Result<String> {
    let url = reqwest::Url::parse_with_params(
        AUTH_ENDPOINT,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .context("could not build the authorization URL")?;
    Ok(url.into())
}

/// Turn the token endpoint's answer into stored tokens. `expires_in` is a duration from
/// now; the store keeps a deadline, so the conversion happens here where `now` is known.
fn tokens_from_response(body: &str, now: i64, previous_refresh: Option<&str>) -> Result<Tokens> {
    let response: TokenResponse =
        serde_json::from_str(body).context("the token endpoint returned something unreadable")?;
    Ok(Tokens {
        access_token: response.access_token,
        // Carry the existing refresh token forward when Google declines to reissue one, or
        // a routine refresh would throw away the only durable credential we hold.
        refresh_token: response
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_string)),
        expires_at: now + response.expires_in,
    })
}

/// `invalid_grant` is Google's answer for a refresh token that has been revoked, expired, or
/// belongs to a deleted client. It is the only one that means "stop retrying and ask the
/// user"; everything else is worth another attempt.
fn is_permanent_rejection(body: &str) -> Option<String> {
    let parsed: ErrorResponse = serde_json::from_str(body).ok()?;
    (parsed.error == "invalid_grant").then(|| {
        parsed
            .error_description
            .unwrap_or_else(|| parsed.error.clone())
    })
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// A loopback listener on a port the OS picks. Google's desktop clients need no redirect URI
/// registered in the console, so the port can differ on every attempt.
pub struct Loopback {
    server: tiny_http::Server,
    pub redirect_uri: String,
}

impl Loopback {
    pub fn bind() -> Result<Self> {
        let server = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|error| anyhow::anyhow!("could not listen on the loopback: {error}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .context("the loopback listener has no IP address")?
            .port();
        Ok(Self {
            server,
            redirect_uri: format!("http://127.0.0.1:{port}"),
        })
    }

    /// Block until the browser delivers the redirect. Requests that are not it — the
    /// browser's unprompted favicon fetch, above all — are answered and ignored rather than
    /// mistaken for a failed login.
    pub fn wait_for_redirect(&self, state: &str, timeout: Duration) -> Result<Redirect> {
        let deadline = SystemTime::now() + timeout;
        loop {
            let remaining = deadline
                .duration_since(SystemTime::now())
                .unwrap_or_default();
            if remaining.is_zero() {
                bail!("timed out waiting for the browser to come back from the consent screen");
            }

            let Some(request) = self
                .server
                .recv_timeout(remaining)
                .context("the loopback listener failed")?
            else {
                continue;
            };

            let outcome = parse_redirect(request.url(), state);
            let page = match &outcome {
                Redirect::Code(_) => "<h1>agenda is connected.</h1><p>You can close this tab.</p>",
                Redirect::Denied { .. } => "<h1>Not connected.</h1><p>You declined access.</p>",
                Redirect::StateMismatch => "<h1>Refused.</h1><p>That redirect did not match.</p>",
                Redirect::Ignored => "",
            };
            let _ = request.respond(
                tiny_http::Response::from_string(page).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..])
                        .expect("a literal header is always valid"),
                ),
            );

            if !matches!(outcome, Redirect::Ignored) {
                return Ok(outcome);
            }
        }
    }
}

/// Exchange the authorization code. Runs once per account, at connect time.
pub async fn exchange_code(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
    pkce: &Pkce,
    redirect_uri: &str,
) -> Result<Tokens> {
    let response = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("code_verifier", &pkce.verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .context("could not reach Google's token endpoint")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("could not read the token response")?;
    if !status.is_success() {
        bail!("Google refused the authorization code ({status}): {body}");
    }
    tokens_from_response(&body, now_unix(), None)
}

/// Trade a refresh token for a fresh access token.
///
/// The error type is deliberately `anyhow::Error` carrying a `RefreshRejected` rather than a
/// bespoke enum: the caller branches with `downcast_ref`, and every other failure here is a
/// transient one it should treat alike.
pub async fn refresh(
    client: &reqwest::Client,
    token_endpoint: &str,
    account: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<Tokens> {
    let response = client
        .post(token_endpoint)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .context("could not reach Google's token endpoint")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("could not read the refresh response")?;

    if !status.is_success() {
        if let Some(reason) = is_permanent_rejection(&body) {
            return Err(RefreshRejected {
                account: account.to_string(),
                reason,
            }
            .into());
        }
        bail!("refreshing the token for {account} failed ({status}): {body}");
    }
    tokens_from_response(&body, now_unix(), Some(refresh_token))
}

/// Generate a fresh PKCE pair and CSRF state for one attempt.
pub fn begin() -> Result<(Pkce, String)> {
    Ok((Pkce::generate()?, random_token()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkce() -> Pkce {
        Pkce::generate().unwrap()
    }

    #[test]
    fn the_authorization_url_asks_for_a_refresh_token() {
        // Without both of these Google issues no refresh token, and every restart means
        // signing in again.
        let url = authorization_url("client", "http://127.0.0.1:1234", &pkce(), "state").unwrap();
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn the_authorization_url_carries_the_challenge_and_not_the_verifier() {
        let pkce = pkce();
        let url = authorization_url("client", "http://127.0.0.1:1234", &pkce, "state").unwrap();
        assert!(url.contains(&pkce.challenge));
        assert!(
            !url.contains(&pkce.verifier),
            "the verifier in a URL is the whole weakness S256 exists to close"
        );
        assert!(url.contains("code_challenge_method=S256"));
    }

    #[test]
    fn the_authorization_url_percent_encodes_the_scope() {
        let url = authorization_url("client", "http://127.0.0.1:1234", &pkce(), "state").unwrap();
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar"));
    }

    #[test]
    fn the_scope_asks_for_the_profile_and_nothing_further() {
        // A scope list that drifts wider than it needs to is the kind of thing a user
        // notices on the consent screen and rightly distrusts.
        let scopes: Vec<&str> = SCOPE.split_whitespace().collect();
        assert_eq!(
            scopes,
            vec![
                "https://www.googleapis.com/auth/calendar",
                "https://www.googleapis.com/auth/userinfo.profile",
            ]
        );
    }

    #[test]
    fn a_token_response_becomes_a_deadline_rather_than_a_duration() {
        let tokens = tokens_from_response(
            r#"{"access_token":"ya29.a0","expires_in":3599,"refresh_token":"1//abc","scope":"https://www.googleapis.com/auth/calendar","token_type":"Bearer"}"#,
            1_700_000_000,
            None,
        )
        .unwrap();
        assert_eq!(tokens.access_token, "ya29.a0");
        assert_eq!(tokens.refresh_token.as_deref(), Some("1//abc"));
        assert_eq!(tokens.expires_at, 1_700_003_599);
    }

    #[test]
    fn a_refresh_that_reissues_no_refresh_token_keeps_the_one_we_hold() {
        // Google omits refresh_token on an ordinary refresh. Taking it at face value would
        // throw away the only durable credential the account has.
        let tokens = tokens_from_response(
            r#"{"access_token":"ya29.new","expires_in":3599,"scope":"https://www.googleapis.com/auth/calendar","token_type":"Bearer"}"#,
            1_700_000_000,
            Some("1//the-one-we-already-have"),
        )
        .unwrap();
        assert_eq!(
            tokens.refresh_token.as_deref(),
            Some("1//the-one-we-already-have")
        );
    }

    #[test]
    fn a_reissued_refresh_token_replaces_the_old_one() {
        let tokens = tokens_from_response(
            r#"{"access_token":"ya29.new","expires_in":3599,"refresh_token":"1//rotated","token_type":"Bearer"}"#,
            1_700_000_000,
            Some("1//old"),
        )
        .unwrap();
        assert_eq!(tokens.refresh_token.as_deref(), Some("1//rotated"));
    }

    #[test]
    fn invalid_grant_is_recognised_as_permanent() {
        let reason = is_permanent_rejection(
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        );
        assert_eq!(
            reason.as_deref(),
            Some("Token has been expired or revoked.")
        );
    }

    #[test]
    fn a_transient_failure_is_not_mistaken_for_a_revoked_token() {
        // Parking an account on "Reconnect" because Google was briefly unavailable would
        // make the user re-consent for nothing.
        assert!(is_permanent_rejection(r#"{"error":"internal_failure"}"#).is_none());
        assert!(is_permanent_rejection("<html>502 Bad Gateway</html>").is_none());
    }

    #[test]
    fn a_rejected_refresh_says_which_account_must_be_reconnected() {
        // With several accounts connected, an error that does not name one is useless.
        let rejected = RefreshRejected {
            account: "work@example.com".to_string(),
            reason: "Token has been expired or revoked.".to_string(),
        };
        assert!(rejected.to_string().contains("work@example.com"));
    }

    #[test]
    fn the_loopback_binds_a_port_the_os_chose() {
        let loopback = Loopback::bind().unwrap();
        assert!(loopback.redirect_uri.starts_with("http://127.0.0.1:"));
        assert_ne!(loopback.redirect_uri, "http://127.0.0.1:0");
    }

    #[test]
    fn two_loopbacks_do_not_fight_over_a_port() {
        let first = Loopback::bind().unwrap();
        let second = Loopback::bind().unwrap();
        assert_ne!(first.redirect_uri, second.redirect_uri);
    }
}
