//! Bringing a calendar's events into the store.
//!
//! Sync writes only server-owned columns. It never touches `accounts`, nor
//! `calendars.visible` or `calendars.user_color` — the store makes that structural, and this
//! module has no way to reach them.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono_tz::{Tz, UTC};

use crate::google::wire::{Mapped, map_event};
use crate::google::{Session, SyncTokenGone};
use crate::store::{Calendar, Store};

/// What one calendar's sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub stored: usize,
    pub deleted: usize,
    /// The cursor to send next time. `None` means the next run must sync in full.
    pub sync_token: Option<String>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// A calendar's zone, or UTC when Google names one this build of `chrono-tz` does not know.
///
/// Falling back rather than failing: an unrecognised zone name would otherwise cost the user
/// the whole calendar, and UTC puts all-day events at worst a few hours out rather than
/// showing nothing at all.
fn zone_of(name: Option<&str>) -> Tz {
    match name.and_then(|name| name.parse::<Tz>().ok()) {
        Some(zone) => zone,
        None => {
            if let Some(name) = name {
                tracing::warn!(zone = name, "unknown timezone; falling back to UTC");
            }
            UTC
        }
    }
}

/// Fetch every event on a calendar and write it into the store.
///
/// Pages are applied as they arrive rather than collected first: a calendar with years of
/// history should not have to fit in memory before any of it is usable.
pub async fn full_sync(
    session: &mut Session,
    store: &Arc<Mutex<Store>>,
    calendar_id: &str,
    calendar_timezone: Option<&str>,
) -> Result<SyncReport> {
    run(session, store, calendar_id, calendar_timezone, None).await
}

/// Bring one calendar up to date, using its stored cursor if it has one.
///
/// A cursor Google refuses is not an error the user should ever see (SPEC §2.1): the cache
/// is dropped and the calendar refetched in full, and the only trace is a debug line.
pub async fn sync_calendar(
    session: &mut Session,
    store: &Arc<Mutex<Store>>,
    calendar: &Calendar,
) -> Result<SyncReport> {
    let zone = calendar.timezone.as_deref();
    let Some(cursor) = calendar.sync_token.as_deref() else {
        return full_sync(session, store, &calendar.id, zone).await;
    };

    match run(session, store, &calendar.id, zone, Some(cursor)).await {
        Ok(report) => Ok(report),
        Err(error) if error.downcast_ref::<SyncTokenGone>().is_some() => {
            tracing::debug!(
                account = %session.account,
                calendar = %calendar.id,
                "the sync token was refused; resyncing this calendar in full"
            );
            // Dropped before the refetch, so an interrupted resync leaves the calendar
            // marked as needing one rather than trusting a cursor that no longer works.
            {
                let store = store
                    .lock()
                    .map_err(|_| anyhow::anyhow!("the store lock was poisoned"))?;
                store.set_sync_token(&session.account, &calendar.id, None, now_unix())?;
                store.clear_calendar(&session.account, &calendar.id)?;
            }
            full_sync(session, store, &calendar.id, zone).await
        }
        Err(error) => Err(error),
    }
}

/// Sync every visible calendar on an account, one failure at a time.
///
/// SPEC §2.11: one calendar erroring must not cost the account the rest, and one account's
/// dead token must not blank the whole calendar. Errors are collected and reported, never
/// propagated in a way that abandons the remaining work.
pub async fn sync_account(
    session: &mut Session,
    store: &Arc<Mutex<Store>>,
) -> Result<AccountReport> {
    let calendars = store
        .lock()
        .map_err(|_| anyhow::anyhow!("the store lock was poisoned"))?
        .calendars(&session.account)?;

    let mut report = AccountReport::default();
    for calendar in calendars {
        match sync_calendar(session, store, &calendar).await {
            Ok(one) => {
                report.stored += one.stored;
                report.deleted += one.deleted;
                report.synced += 1;
            }
            Err(error) => {
                tracing::warn!(
                    account = %session.account,
                    calendar = %calendar.id,
                    error = %format!("{error:#}"),
                    "a calendar failed to sync; continuing with the rest"
                );
                report.failures.push((calendar.id.clone(), error));
            }
        }
    }
    Ok(report)
}

/// What one account's sync run did, including what went wrong without stopping it.
#[derive(Debug, Default)]
pub struct AccountReport {
    pub synced: usize,
    pub stored: usize,
    pub deleted: usize,
    pub failures: Vec<(String, anyhow::Error)>,
}

impl AccountReport {
    /// Whether the account needs the user to reconnect it — as opposed to a calendar or two
    /// having had a bad minute.
    pub fn needs_reconnect(&self) -> bool {
        self.failures.iter().any(|(_, error)| {
            error
                .downcast_ref::<crate::auth::RefreshRejected>()
                .is_some()
        })
    }
}

async fn run(
    session: &mut Session,
    store: &Arc<Mutex<Store>>,
    calendar_id: &str,
    calendar_timezone: Option<&str>,
    sync_token: Option<&str>,
) -> Result<SyncReport> {
    let account = session.account.clone();
    let mut report = SyncReport::default();
    let mut page_token: Option<String> = None;
    let mut zone = zone_of(calendar_timezone);

    loop {
        let page = session
            .events_page(
                calendar_id,
                page_token.as_deref(),
                // Only on the first request: Google rejects a syncToken sent alongside a
                // pageToken, and the page token already carries the delta's position.
                page_token.is_none().then_some(sync_token).flatten(),
            )
            .await?;

        // The response states the calendar's own zone; prefer it over whatever we were told.
        if let Some(name) = page.time_zone.as_deref() {
            zone = zone_of(Some(name));
        }

        {
            let store = store
                .lock()
                .map_err(|_| anyhow::anyhow!("the store lock was poisoned"))?;
            for wire in &page.items {
                match map_event(&account, calendar_id, zone, wire)
                    .with_context(|| format!("could not read the event {}", wire.id))?
                {
                    Mapped::Store(event) => {
                        store.upsert_event(&event)?;
                        report.stored += 1;
                    }
                    Mapped::Cancelled { id } => {
                        store.delete_event(&account, calendar_id, &id)?;
                        report.deleted += 1;
                    }
                }
            }
        }

        report.sync_token = page.next_sync_token.clone();
        match page.next_page_token {
            // A page pointing at itself would loop forever. Google does not do this, but an
            // infinite request loop is not a failure mode worth risking.
            Some(next) if Some(&next) != page_token.as_ref() => page_token = Some(next),
            _ => break,
        }
    }

    store
        .lock()
        .map_err(|_| anyhow::anyhow!("the store lock was poisoned"))?
        .set_sync_token(
            &account,
            calendar_id,
            report.sync_token.as_deref(),
            now_unix(),
        )?;

    tracing::debug!(
        %account,
        calendar_id,
        stored = report.stored,
        deleted = report.deleted,
        "synced a calendar"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::auth::keyring::Tokens;
    use crate::config::Credentials;
    use crate::store::CalendarMetadata;

    const PAGE1: &str = include_str!("../../tests/fixtures/events_page1.json");
    const PAGE2: &str = include_str!("../../tests/fixtures/events_page2.json");

    pub(super) struct FakeGoogle {
        base: String,
        pub(super) hits: Arc<AtomicUsize>,
        /// Every request URL, in order. Google rejects some parameter combinations, and a
        /// stand-in that accepts anything would let those through to production.
        pub(super) urls: Arc<Mutex<Vec<String>>>,
    }

    pub(super) fn serve(responses: Vec<(u16, String)>) -> FakeGoogle {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let urls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let counter = hits.clone();
        let seen = urls.clone();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                seen.lock().unwrap().push(request.url().to_string());
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let (status, body) = responses
                    .get(index)
                    .cloned()
                    .unwrap_or((500, "no more scripted responses".to_string()));
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(tiny_http::StatusCode(status)),
                );
            }
        });
        FakeGoogle {
            base: format!("http://127.0.0.1:{port}"),
            hits,
            urls,
        }
    }

    /// Two accounts, one of them holding two calendars (SPEC §8).
    pub(super) fn store_with_two_accounts() -> Arc<Mutex<Store>> {
        let mut store = Store::open_in_memory().unwrap();
        for (email, color) in [
            ("work@example.com", "#e66100"),
            ("personal@example.com", "#3584e4"),
        ] {
            store
                .connect_account(
                    email,
                    color,
                    1,
                    &[CalendarMetadata {
                        account: email.to_string(),
                        id: "primary".to_string(),
                        summary: email.to_string(),
                        color: None,
                        timezone: Some("Europe/Madrid".to_string()),
                        access_role: "owner".to_string(),
                        is_primary: true,
                    }],
                )
                .unwrap();
        }
        store
            .upsert_calendar(&CalendarMetadata {
                account: "work@example.com".to_string(),
                id: "team".to_string(),
                summary: "Team".to_string(),
                color: None,
                timezone: Some("Europe/Madrid".to_string()),
                access_role: "reader".to_string(),
                is_primary: false,
            })
            .unwrap();
        Arc::new(Mutex::new(store))
    }

    pub(super) fn session(fake: &FakeGoogle, account: &str) -> Session {
        Session::for_tests(
            account,
            Credentials {
                client_id: "id".to_string(),
                client_secret: "secret".to_string(),
            },
            Tokens {
                access_token: "ya29".to_string(),
                refresh_token: Some("1//r".to_string()),
                expires_at: i64::MAX,
            },
            &fake.base,
            &format!("{}/token", fake.base),
        )
    }

    #[tokio::test]
    async fn a_full_sync_stores_every_event_across_every_page() {
        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");

        let report = full_sync(&mut session, &store, "primary", Some("Europe/Madrid"))
            .await
            .unwrap();

        assert_eq!(
            fake.hits.load(Ordering::SeqCst),
            2,
            "pagination must exhaust"
        );
        assert_eq!(
            report.stored, 6,
            "five on the first page, one on the second"
        );
        assert_eq!(report.deleted, 1, "and one cancellation applied");
    }

    #[tokio::test]
    async fn a_cancelled_event_removes_the_row_it_names() {
        let store = store_with_two_accounts();
        {
            // A row that a later sync will be told to delete.
            let locked = store.lock().unwrap();
            let mut event = crate::store::Event {
                account: "work@example.com".to_string(),
                calendar_id: "primary".to_string(),
                id: "deleted001".to_string(),
                ical_uid: Some("deleted001@google.com".to_string()),
                etag: None,
                summary: "About to be deleted in Google".to_string(),
                description: None,
                location: None,
                start_utc: 1_789_371_000,
                end_utc: 1_789_371_900,
                timezone: None,
                all_day: false,
                rrule: None,
                recurring_event_id: None,
                original_start_utc: None,
                status: "confirmed".to_string(),
                updated_at: None,
            };
            locked.upsert_event(&event).unwrap();
            event.id = "survivor".to_string();
            locked.upsert_event(&event).unwrap();
        }

        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let mut session = session(&fake, "work@example.com");
        full_sync(&mut session, &store, "primary", Some("Europe/Madrid"))
            .await
            .unwrap();

        let locked = store.lock().unwrap();
        let remaining = locked.events_in_range(0, i64::MAX).unwrap();
        assert!(
            !remaining.iter().any(|event| event.id == "deleted001"),
            "a deletion in Google must disappear locally"
        );
        assert!(
            remaining.iter().any(|event| event.id == "survivor"),
            "and must take nothing else with it"
        );
    }

    #[tokio::test]
    async fn the_sync_token_from_the_last_page_is_recorded_for_next_time() {
        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");
        full_sync(&mut session, &store, "primary", Some("Europe/Madrid"))
            .await
            .unwrap();

        let calendar = store
            .lock()
            .unwrap()
            .calendar("work@example.com", "primary")
            .unwrap()
            .unwrap();
        assert_eq!(
            calendar.sync_token.as_deref(),
            Some("CJDFnbHF7YkDEJDFnbHF7YkDGAUg")
        );
        assert!(calendar.synced_at.is_some());
    }

    #[tokio::test]
    async fn syncing_one_calendar_leaves_the_other_accounts_events_alone() {
        // Account isolation is the whole point of the (account, calendar_id) keying.
        let store = store_with_two_accounts();
        {
            let locked = store.lock().unwrap();
            locked
                .upsert_event(&crate::store::Event {
                    account: "personal@example.com".to_string(),
                    calendar_id: "primary".to_string(),
                    id: "deleted001".to_string(),
                    ical_uid: Some("coincidence@google.com".to_string()),
                    etag: None,
                    summary: "Same id, different account".to_string(),
                    description: None,
                    location: None,
                    start_utc: 1_789_371_000,
                    end_utc: 1_789_371_900,
                    timezone: None,
                    all_day: false,
                    rrule: None,
                    recurring_event_id: None,
                    original_start_utc: None,
                    status: "confirmed".to_string(),
                    updated_at: None,
                })
                .unwrap();
        }

        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let mut session = session(&fake, "work@example.com");
        full_sync(&mut session, &store, "primary", Some("Europe/Madrid"))
            .await
            .unwrap();

        let locked = store.lock().unwrap();
        let survivors = locked.events_in_range(0, i64::MAX).unwrap();
        assert!(
            survivors
                .iter()
                .any(|event| event.account == "personal@example.com" && event.id == "deleted001"),
            "the other account's identically-named event must survive"
        );
    }

    #[tokio::test]
    async fn a_sync_never_touches_the_users_colour_or_visibility() {
        // The §4 guarantee, exercised through the real sync path rather than a bare upsert.
        let store = store_with_two_accounts();
        store
            .lock()
            .unwrap()
            .set_user_style_for_tests("work@example.com", "primary", false, Some("#ff0000"))
            .unwrap();

        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let mut session = session(&fake, "work@example.com");
        full_sync(&mut session, &store, "primary", Some("Europe/Madrid"))
            .await
            .unwrap();

        let calendar = store
            .lock()
            .unwrap()
            .calendar("work@example.com", "primary")
            .unwrap()
            .unwrap();
        assert!(!calendar.visible, "sync must not unhide a calendar");
        assert_eq!(calendar.user_color.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn an_unknown_timezone_falls_back_to_utc_rather_than_losing_the_calendar() {
        assert_eq!(zone_of(Some("Mars/Olympus_Mons")), UTC);
        assert_eq!(zone_of(None), UTC);
        assert_eq!(zone_of(Some("Europe/Madrid")), chrono_tz::Europe::Madrid);
    }
}

#[cfg(test)]
mod incremental_tests {
    use super::tests::{serve, session, store_with_two_accounts};
    use super::*;
    use std::sync::atomic::Ordering;

    const PAGE1: &str = include_str!("../../tests/fixtures/events_page1.json");
    const PAGE2: &str = include_str!("../../tests/fixtures/events_page2.json");
    const DELTA: &str = include_str!("../../tests/fixtures/events_delta.json");
    const GONE: &str = r#"{"error":{"code":410,"message":"Sync token is no longer valid"}}"#;

    fn calendar(store: &Arc<Mutex<Store>>, account: &str, id: &str) -> Calendar {
        store
            .lock()
            .unwrap()
            .calendar(account, id)
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn a_calendar_with_no_cursor_syncs_in_full() {
        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");

        let before = calendar(&store, "work@example.com", "primary");
        assert_eq!(before.sync_token, None, "a fresh calendar has no cursor");

        sync_calendar(&mut session, &store, &before).await.unwrap();
        assert_eq!(fake.hits.load(Ordering::SeqCst), 2);
        assert!(
            calendar(&store, "work@example.com", "primary")
                .sync_token
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_delta_applies_an_update_an_insert_and_a_deletion() {
        let fake = serve(vec![
            (200, PAGE1.to_string()),
            (200, PAGE2.to_string()),
            (200, DELTA.to_string()),
        ]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");

        let fresh = calendar(&store, "work@example.com", "primary");
        sync_calendar(&mut session, &store, &fresh).await.unwrap();

        let with_cursor = calendar(&store, "work@example.com", "primary");
        let report = sync_calendar(&mut session, &store, &with_cursor)
            .await
            .unwrap();

        assert_eq!(report.stored, 2, "one updated, one new");
        assert_eq!(report.deleted, 1);

        let events = store.lock().unwrap().events_in_range(0, i64::MAX).unwrap();
        let standup = events.iter().find(|e| e.id == "timed001").unwrap();
        assert_eq!(standup.summary, "Standup (renamed)", "the update applied");
        assert!(
            events.iter().any(|e| e.id == "brandnew001"),
            "the insert applied"
        );
        assert!(
            !events.iter().any(|e| e.id == "overnight001"),
            "the deletion applied"
        );
        assert!(
            events.iter().any(|e| e.id == "master001"),
            "a delta must not disturb events it did not mention"
        );
    }

    #[tokio::test]
    async fn the_cursor_advances_to_the_one_the_delta_returned() {
        let fake = serve(vec![
            (200, PAGE1.to_string()),
            (200, PAGE2.to_string()),
            (200, DELTA.to_string()),
        ]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");
        let fresh = calendar(&store, "work@example.com", "primary");
        sync_calendar(&mut session, &store, &fresh).await.unwrap();
        let with_cursor = calendar(&store, "work@example.com", "primary");
        sync_calendar(&mut session, &store, &with_cursor)
            .await
            .unwrap();

        assert_eq!(
            calendar(&store, "work@example.com", "primary")
                .sync_token
                .as_deref(),
            Some("CJDFnbHF7YkDEJDFnbHF7YkDGAUgAFTER")
        );
    }

    #[tokio::test]
    async fn a_refused_cursor_resyncs_in_full_without_surfacing_an_error() {
        // SPEC §2.1: a 410 on a stale token silently triggers a full resync rather than
        // surfacing an error.
        let fake = serve(vec![
            (410, GONE.to_string()),
            (200, PAGE1.to_string()),
            (200, PAGE2.to_string()),
        ]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");
        store
            .lock()
            .unwrap()
            .set_sync_token("work@example.com", "primary", Some("STALE"), 1)
            .unwrap();

        let stale = calendar(&store, "work@example.com", "primary");
        let report = sync_calendar(&mut session, &store, &stale)
            .await
            .expect("a refused cursor is recovered from, not reported");

        assert_eq!(report.stored, 6, "the full resync ran");
        assert_eq!(
            calendar(&store, "work@example.com", "primary")
                .sync_token
                .as_deref(),
            Some("CJDFnbHF7YkDEJDFnbHF7YkDGAUg"),
            "and left a usable cursor behind"
        );
    }

    #[tokio::test]
    async fn a_resync_after_a_refused_cursor_drops_the_stale_events_first() {
        // Without the clear, an event deleted in Google while the cursor was stale would
        // survive forever: the full resync never mentions it again.
        let store = store_with_two_accounts();
        store
            .lock()
            .unwrap()
            .upsert_event(&crate::store::Event {
                account: "work@example.com".to_string(),
                calendar_id: "primary".to_string(),
                id: "vanished_while_we_were_not_looking".to_string(),
                ical_uid: Some("ghost@google.com".to_string()),
                etag: None,
                summary: "Deleted in Google while the cursor was stale".to_string(),
                description: None,
                location: None,
                start_utc: 1_789_371_000,
                end_utc: 1_789_371_900,
                timezone: None,
                all_day: false,
                rrule: None,
                recurring_event_id: None,
                original_start_utc: None,
                status: "confirmed".to_string(),
                updated_at: None,
            })
            .unwrap();
        store
            .lock()
            .unwrap()
            .set_sync_token("work@example.com", "primary", Some("STALE"), 1)
            .unwrap();

        let fake = serve(vec![
            (410, GONE.to_string()),
            (200, PAGE1.to_string()),
            (200, PAGE2.to_string()),
        ]);
        let mut session = session(&fake, "work@example.com");
        let stale = calendar(&store, "work@example.com", "primary");
        sync_calendar(&mut session, &store, &stale).await.unwrap();

        let events = store.lock().unwrap().events_in_range(0, i64::MAX).unwrap();
        assert!(
            !events
                .iter()
                .any(|e| e.id == "vanished_while_we_were_not_looking"),
            "a resync must not leave a ghost behind"
        );
    }

    #[tokio::test]
    async fn a_paginated_delta_sends_the_sync_token_only_on_the_first_request() {
        // Google refuses a request carrying both syncToken and pageToken. Sending both would
        // fail every delta that happens to span more than one page — which is exactly the
        // busy calendar the user would notice.
        const DELTA_P1: &str = include_str!("../../tests/fixtures/events_delta_page1.json");
        const DELTA_P2: &str = include_str!("../../tests/fixtures/events_delta_page2.json");

        let fake = serve(vec![
            (200, DELTA_P1.to_string()),
            (200, DELTA_P2.to_string()),
        ]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");
        store
            .lock()
            .unwrap()
            .set_sync_token("work@example.com", "primary", Some("A_GOOD_CURSOR"), 1)
            .unwrap();

        let with_cursor = calendar(&store, "work@example.com", "primary");
        sync_calendar(&mut session, &store, &with_cursor)
            .await
            .unwrap();

        let urls = fake.urls.lock().unwrap().clone();
        assert_eq!(urls.len(), 2, "the delta spanned two pages");
        assert!(
            urls[0].contains("syncToken=A_GOOD_CURSOR"),
            "first request: {}",
            urls[0]
        );
        assert!(!urls[0].contains("pageToken"), "first request: {}", urls[0]);
        assert!(
            urls[1].contains("pageToken=DELTAPAGE2"),
            "second request: {}",
            urls[1]
        );
        assert!(
            !urls[1].contains("syncToken"),
            "Google rejects syncToken alongside pageToken: {}",
            urls[1]
        );
    }

    #[tokio::test]
    async fn every_request_asks_for_deletions_and_leaves_expansion_to_us() {
        // showDeleted is what makes a deletion visible at all; singleEvents must stay off or
        // Google expands series server-side and the RRULE is lost.
        let fake = serve(vec![(200, PAGE1.to_string()), (200, PAGE2.to_string())]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");
        let fresh = calendar(&store, "work@example.com", "primary");
        sync_calendar(&mut session, &store, &fresh).await.unwrap();

        for url in fake.urls.lock().unwrap().iter() {
            assert!(url.contains("showDeleted=true"), "{url}");
            assert!(!url.contains("singleEvents"), "{url}");
        }
    }

    #[tokio::test]
    async fn one_calendar_failing_does_not_cost_the_account_the_others() {
        // work@example.com holds two calendars. The first fails; the second must still sync.
        let fake = serve(vec![
            (500, "primary is having a bad minute".to_string()),
            (200, PAGE1.to_string()),
            (200, PAGE2.to_string()),
        ]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");

        let report = sync_account(&mut session, &store).await.unwrap();

        assert_eq!(report.failures.len(), 1, "one calendar failed");
        assert_eq!(report.synced, 1, "and the other still synced");
        assert!(report.stored > 0);
        assert!(!report.needs_reconnect(), "a 500 is not a revoked token");
    }

    #[tokio::test]
    async fn a_revoked_token_marks_the_account_for_reconnection() {
        // SPEC §2.11 in miniature: this is what tells the UI to show Reconnect for this
        // account and nothing else.
        let fake = serve(vec![
            (401, "expired".to_string()),
            (401, "still expired".to_string()),
            (401, "expired".to_string()),
            (401, "still expired".to_string()),
        ]);
        let store = store_with_two_accounts();
        let mut session = session(&fake, "work@example.com");
        session.tokens.refresh_token = None;

        let report = sync_account(&mut session, &store).await.unwrap();
        assert!(report.needs_reconnect());
        assert_eq!(report.synced, 0);
    }
}
