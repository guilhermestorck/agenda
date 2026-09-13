//! The authorised half of the Google Calendar client.
//!
//! One `Session` per connected account, holding that account's tokens. Accounts are
//! deliberately independent: SPEC §2.11 requires a revoked token to park one account on
//! "Reconnect" while the others keep syncing, and sharing a session would make that
//! impossible.

use anyhow::{Context, Result, bail};

use crate::auth::{self, RefreshRejected};
use crate::config::Credentials;
use crate::store::CalendarMetadata;

use super::wire::{CalendarListEntry, CalendarListPage, EventsPage, calendar_metadata};

pub const API_BASE: &str = "https://www.googleapis.com/calendar/v3";
/// OpenID Connect's userinfo endpoint, reachable with the `userinfo.profile` scope.
pub const USERINFO_ENDPOINT: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// Google refused a `syncToken`: the cached cursor is too old to extend, and the only way
/// forward is to drop it and resync in full. Typed because the caller must branch on it
/// (SPEC §7) — it changes control flow rather than merely describing a failure, and §2.1
/// requires it to be handled silently rather than surfaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncTokenGone {
    pub resource: String,
}

impl std::fmt::Display for SyncTokenGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the sync token for {} is no longer valid; a full resync is required",
            self.resource
        )
    }
}

impl std::error::Error for SyncTokenGone {}

pub struct Session {
    http: reqwest::Client,
    api_base: String,
    token_endpoint: String,
    credentials: Credentials,
    pub account: String,
    pub tokens: auth::keyring::Tokens,
}

impl Session {
    pub fn new(account: &str, credentials: Credentials, tokens: auth::keyring::Tokens) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_base: API_BASE.to_string(),
            token_endpoint: auth::TOKEN_ENDPOINT.to_string(),
            credentials,
            account: account.to_string(),
            tokens,
        }
    }

    #[cfg(test)]
    fn against(mut self, api_base: &str, token_endpoint: &str) -> Self {
        self.api_base = api_base.to_string();
        self.token_endpoint = token_endpoint.to_string();
        self
    }

    /// Point a session at a stand-in server. Test-only, so the real endpoints cannot be
    /// redirected by anything shipped.
    #[cfg(test)]
    pub fn for_tests(
        account: &str,
        credentials: Credentials,
        tokens: auth::keyring::Tokens,
        api_base: &str,
        token_endpoint: &str,
    ) -> Self {
        Self::new(account, credentials, tokens).against(api_base, token_endpoint)
    }

    /// GET an API path, refreshing the access token once if Google rejects it.
    ///
    /// Exactly once: a second rejection means the credentials are not merely stale, and
    /// retrying further would spin against an endpoint that will keep saying no.
    async fn get(&mut self, path: &str, query: &[(&str, String)]) -> Result<String> {
        let mut refreshed = false;
        loop {
            let response = self
                .http
                .get(format!("{}{path}", self.api_base))
                .bearer_auth(&self.tokens.access_token)
                .query(query)
                .send()
                .await
                .with_context(|| format!("could not reach {path}"))?;

            let status = response.status();
            if status.is_success() {
                return response
                    .text()
                    .await
                    .with_context(|| format!("could not read the response from {path}"));
            }

            let body = response.text().await.unwrap_or_default();

            if status == reqwest::StatusCode::GONE {
                return Err(SyncTokenGone {
                    resource: format!("{} {path}", self.account),
                }
                .into());
            }

            if status == reqwest::StatusCode::UNAUTHORIZED && !refreshed {
                self.refresh_access_token().await?;
                refreshed = true;
                continue;
            }

            // Google's own words for this are a page of JSON ending in
            // ACCESS_TOKEN_SCOPE_INSUFFICIENT, which tells a user nothing about what to do.
            // The cause is always the same: the grant does not carry the scope we asked
            // for, and no amount of retrying changes that.
            if status == reqwest::StatusCode::FORBIDDEN
                && body.contains("ACCESS_TOKEN_SCOPE_INSUFFICIENT")
            {
                bail!(
                    "Google granted access without calendar permission. \
                     Check that the consent screen still lists the Calendar scope, and that \
                     every permission box was ticked when approving."
                );
            }

            bail!(
                "{} returned {status} for {}: {body}",
                self.api_base,
                self.account
            );
        }
    }

    async fn refresh_access_token(&mut self) -> Result<()> {
        let refresh_token = self.tokens.refresh_token.clone().ok_or_else(|| {
            // No refresh token and a rejected access token is the same dead end as a
            // revoked one, and the user's next step is identical.
            RefreshRejected {
                account: self.account.clone(),
                reason: "no refresh token is stored for this account".to_string(),
            }
        })?;

        self.tokens = auth::refresh(
            &self.http,
            &self.token_endpoint,
            &self.account,
            &self.credentials.client_id,
            &self.credentials.client_secret,
            &refresh_token,
        )
        .await?;
        Ok(())
    }

    /// Every calendar on the account, following `nextPageToken` to exhaustion.
    pub async fn calendar_list(&mut self) -> Result<Vec<CalendarListEntry>> {
        let mut entries = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let query = match &page_token {
                Some(token) => vec![("pageToken", token.clone())],
                None => Vec::new(),
            };
            let body = self.get("/users/me/calendarList", &query).await?;
            let page: CalendarListPage = serde_json::from_str(&body)
                .context("the calendarList response was not readable")?;
            entries.extend(page.items);
            match page.next_page_token {
                // A page that points at itself would loop forever; Google does not do this,
                // but an infinite request loop is not a failure mode worth risking.
                Some(next) if Some(&next) != page_token.as_ref() => page_token = Some(next),
                _ => return Ok(entries),
            }
        }
    }

    /// One page of a calendar's events.
    ///
    /// `singleEvents` is deliberately *not* set: Google would expand recurring series
    /// server-side, which loses the RRULE and makes a week of a long series hundreds of rows
    /// instead of one. Expansion is ours (SPEC §3).
    pub async fn events_page(
        &mut self,
        calendar_id: &str,
        page_token: Option<&str>,
        sync_token: Option<&str>,
    ) -> Result<EventsPage> {
        let mut query = vec![
            ("maxResults", "2500".to_string()),
            ("showDeleted", "true".to_string()),
        ];
        if let Some(token) = page_token {
            query.push(("pageToken", token.to_string()));
        }
        // Sent only on a delta run. With it, Google returns just what changed — and refuses
        // with 410 if the cursor is too old to extend.
        if let Some(token) = sync_token {
            query.push(("syncToken", token.to_string()));
        }

        let path = format!("/calendars/{}/events", urlencode(calendar_id));
        let body = self.get(&path, &query).await?;
        serde_json::from_str(&body).context("the events response was not readable")
    }

    /// The calendars, as this app's own metadata rather than Google's shape.
    pub async fn calendars(&mut self) -> Result<Vec<CalendarMetadata>> {
        let account = self.account.clone();
        Ok(self
            .calendar_list()
            .await?
            .iter()
            .map(|entry| calendar_metadata(&account, entry))
            .collect())
    }
}

/// Google's profile for the signed-in account.
///
/// Only the two fields that reach the sidebar. Everything else the endpoint returns — `sub`,
/// `given_name`, `locale` — is data about a person we have no use for and no reason to hold.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct UserInfo {
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// Fetch the account's own profile. Never fatal: an account with no picture is an account
/// with a monogram, not a connect that failed.
pub async fn userinfo(
    http: &reqwest::Client,
    endpoint: &str,
    access_token: &str,
) -> Result<UserInfo> {
    let response = http
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .context("could not reach the userinfo endpoint")?;
    if !response.status().is_success() {
        bail!("userinfo returned {}", response.status());
    }
    response
        .json()
        .await
        .context("the userinfo response was not readable")
}

/// Calendar ids are email addresses and group addresses containing `@` and `#`, which must
/// not be taken for path or fragment syntax.
fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PAGE1: &str = include_str!("../../tests/fixtures/calendar_list_page1.json");
    const PAGE2: &str = include_str!("../../tests/fixtures/calendar_list_page2.json");

    /// A stand-in for Google that answers on the loopback. Real HTTP, real reqwest, real
    /// status codes — the retry rule is worth testing as behaviour rather than asserting.
    struct FakeGoogle {
        base: String,
        calendar_hits: Arc<AtomicUsize>,
        token_hits: Arc<AtomicUsize>,
    }

    fn serve(responses: Vec<(u16, String)>) -> FakeGoogle {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let calendar_hits = Arc::new(AtomicUsize::new(0));
        let token_hits = Arc::new(AtomicUsize::new(0));
        let (calendars, tokens) = (calendar_hits.clone(), token_hits.clone());

        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let url = request.url().to_string();
                let (status, body) = if url.starts_with("/token") {
                    tokens.fetch_add(1, Ordering::SeqCst);
                    (
                        200,
                        r#"{"access_token":"ya29.refreshed","expires_in":3599,"token_type":"Bearer"}"#
                            .to_string(),
                    )
                } else {
                    let index = calendars.fetch_add(1, Ordering::SeqCst);
                    responses
                        .get(index)
                        .cloned()
                        .unwrap_or((500, "no more scripted responses".to_string()))
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(tiny_http::StatusCode(status)),
                );
            }
        });

        FakeGoogle {
            base: format!("http://127.0.0.1:{port}"),
            calendar_hits,
            token_hits,
        }
    }

    fn session(fake: &FakeGoogle) -> Session {
        Session::new(
            "work@example.com",
            Credentials {
                client_id: "id".to_string(),
                client_secret: "secret".to_string(),
            },
            auth::keyring::Tokens {
                access_token: "ya29.stale".to_string(),
                refresh_token: Some("1//refresh".to_string()),
                expires_at: 0,
            },
        )
        .against(&fake.base, &format!("{}/token", fake.base))
    }

    #[tokio::test]
    async fn a_profile_is_read_from_the_userinfo_endpoint() {
        let fake = serve(vec![(
            200,
            r#"{"sub":"1","name":"Guilherme","given_name":"Guilherme","picture":"https://lh3.googleusercontent.com/a/x=s96-c","locale":"en"}"#
                .to_string(),
        )]);
        let profile = userinfo(&reqwest::Client::new(), &fake.base, "ya29")
            .await
            .unwrap();
        assert_eq!(profile.name.as_deref(), Some("Guilherme"));
        assert!(profile.picture.as_deref().unwrap().starts_with("https://"));
    }

    #[tokio::test]
    async fn a_profile_with_no_picture_is_still_a_profile() {
        let fake = serve(vec![(
            200,
            r#"{"sub":"1","name":"No Picture"}"#.to_string(),
        )]);
        let profile = userinfo(&reqwest::Client::new(), &fake.base, "ya29")
            .await
            .unwrap();
        assert_eq!(profile.name.as_deref(), Some("No Picture"));
        assert_eq!(profile.picture, None);
    }

    #[test]
    fn a_calendar_id_is_escaped_before_it_becomes_a_path_segment() {
        // Calendar ids are addresses: they carry '@', and holiday calendars carry '#'.
        // Left raw, the '#' truncates the request at a fragment and the call hits the wrong
        // endpoint entirely.
        assert_eq!(urlencode("work@example.com"), "work%40example.com");
        assert_eq!(
            urlencode("es.spanish#holiday@group.v.calendar.google.com"),
            "es.spanish%23holiday%40group.v.calendar.google.com"
        );
        assert_eq!(urlencode("primary"), "primary");
    }

    #[tokio::test]
    async fn calendars_are_fetched_and_mapped() {
        let fake = serve(vec![(200, PAGE2.to_string())]);
        let calendars = session(&fake).calendars().await.unwrap();
        assert_eq!(calendars.len(), 1);
        assert_eq!(calendars[0].account, "work@example.com");
        assert_eq!(calendars[0].summary, "On the second page");
    }

    #[tokio::test]
    async fn pagination_follows_the_next_page_token_to_exhaustion() {
        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let calendars = session(&fake).calendars().await.unwrap();
        assert_eq!(
            calendars.len(),
            4,
            "three on the first page, one on the second"
        );
        assert_eq!(fake.calendar_hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_rejected_access_token_is_refreshed_once_and_the_request_retried() {
        let fake = serve(vec![(401, "expired".to_string()), (200, PAGE2.to_string())]);
        let mut session = session(&fake);
        let calendars = session.calendars().await.unwrap();

        assert_eq!(calendars.len(), 1, "the retry must succeed transparently");
        assert_eq!(fake.token_hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            session.tokens.access_token, "ya29.refreshed",
            "the fresh token must be kept, or the next call refreshes again"
        );
    }

    #[tokio::test]
    async fn a_second_rejection_gives_up_rather_than_refreshing_again() {
        // Spinning here would hammer Google's token endpoint for an account that will keep
        // saying no.
        let fake = serve(vec![
            (401, "nope".to_string()),
            (401, "still nope".to_string()),
        ]);
        let error = session(&fake).calendars().await.unwrap_err();

        assert_eq!(
            fake.token_hits.load(Ordering::SeqCst),
            1,
            "exactly one refresh"
        );
        assert_eq!(
            fake.calendar_hits.load(Ordering::SeqCst),
            2,
            "exactly one retry"
        );
        assert!(error.to_string().contains("401"));
    }

    #[tokio::test]
    async fn a_missing_scope_says_what_to_do_rather_than_quoting_google() {
        // The real payload that cost a failed connect on 2026-09-13.
        let fake = serve(vec![(
            403,
            r#"{"error":{"code":403,"message":"Request had insufficient authentication scopes.","status":"PERMISSION_DENIED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"ACCESS_TOKEN_SCOPE_INSUFFICIENT"}]}}"#
                .to_string(),
        )]);
        let error = session(&fake).calendars().await.unwrap_err().to_string();

        assert!(error.contains("calendar permission"), "got: {error}");
        assert!(error.contains("consent screen"), "got: {error}");
        assert!(
            !error.contains("ACCESS_TOKEN_SCOPE_INSUFFICIENT"),
            "the raw reason code helps nobody: {error}"
        );
    }

    #[tokio::test]
    async fn another_403_is_still_reported_in_full() {
        // Only the scope case gets a rewritten message; anything else keeps Google's text,
        // which is the only clue there is.
        let fake = serve(vec![(
            403,
            r#"{"error":{"code":403,"message":"Rate Limit Exceeded"}}"#.to_string(),
        )]);
        let error = session(&fake).calendars().await.unwrap_err().to_string();
        assert!(error.contains("Rate Limit Exceeded"), "got: {error}");
    }

    #[tokio::test]
    async fn a_410_becomes_the_typed_error_that_task_7_branches_on() {
        let fake = serve(vec![(410, r#"{"error":{"code":410}}"#.to_string())]);
        let error = session(&fake).calendars().await.unwrap_err();
        let gone = error
            .downcast_ref::<SyncTokenGone>()
            .expect("410 must arrive as SyncTokenGone, not as a string the caller must match on");
        assert!(gone.resource.contains("work@example.com"));
    }

    #[tokio::test]
    async fn an_account_with_no_refresh_token_is_sent_to_reconnect_not_retried() {
        let fake = serve(vec![(401, "expired".to_string())]);
        let mut session = session(&fake);
        session.tokens.refresh_token = None;
        let error = session.calendars().await.unwrap_err();

        assert_eq!(fake.token_hits.load(Ordering::SeqCst), 0);
        let rejected = error
            .downcast_ref::<RefreshRejected>()
            .expect("the user's next step is to reconnect, which only a typed error can say");
        assert_eq!(rejected.account, "work@example.com");
    }
}
