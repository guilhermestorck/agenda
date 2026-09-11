//! Bringing a calendar's events into the store.
//!
//! Sync writes only server-owned columns. It never touches `accounts`, nor
//! `calendars.visible` or `calendars.user_color` — the store makes that structural, and this
//! module has no way to reach them.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono_tz::{Tz, UTC};

use crate::google::Session;
use crate::google::wire::{Mapped, map_event};
use crate::store::Store;

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
    let account = session.account.clone();
    let mut report = SyncReport::default();
    let mut page_token: Option<String> = None;
    let mut zone = zone_of(calendar_timezone);

    loop {
        let page = session
            .events_page(calendar_id, page_token.as_deref())
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

    struct FakeGoogle {
        base: String,
        hits: Arc<AtomicUsize>,
    }

    fn serve(responses: Vec<(u16, String)>) -> FakeGoogle {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
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
        }
    }

    /// Two accounts, one of them holding two calendars (SPEC §8).
    fn store_with_two_accounts() -> Arc<Mutex<Store>> {
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

    fn session(fake: &FakeGoogle, account: &str) -> Session {
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
