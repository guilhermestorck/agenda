//! Google's JSON, and the one place it is allowed to be shaped like Google's JSON.
//!
//! SPEC §6: wire types stay here, so the rest of the app knows `store::Event` and
//! `store::CalendarMetadata` and a second backend stays possible. Every field is optional
//! that Google omits in practice — a secondary calendar carries no `primary`, a calendar
//! created without one carries no `timeZone` — because a missing field must not fail a sync
//! for every other calendar on the account.

use serde::Deserialize;

use crate::store::CalendarMetadata;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarListPage {
    #[serde(default)]
    pub items: Vec<CalendarListEntry>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarListEntry {
    pub id: String,
    /// Absent on a calendar that has never been given a name.
    pub summary: Option<String>,
    pub background_color: Option<String>,
    pub time_zone: Option<String>,
    /// "owner", "writer", "reader", "freeBusyReader".
    pub access_role: Option<String>,
    /// Present and true only on the account's own primary calendar; omitted otherwise.
    pub primary: Option<bool>,
}

/// Google's entry as this app's calendar metadata.
///
/// Pure, and tested directly: it is the boundary where external data becomes ours, and the
/// only part of the sync engine testable without live credentials (SPEC §8).
pub fn calendar_metadata(account: &str, entry: &CalendarListEntry) -> CalendarMetadata {
    CalendarMetadata {
        account: account.to_string(),
        id: entry.id.clone(),
        // Falling back to the id keeps an unnamed calendar identifiable in the sidebar
        // rather than rendering a blank row.
        summary: entry.summary.clone().unwrap_or_else(|| entry.id.clone()),
        color: entry.background_color.clone(),
        timezone: entry.time_zone.clone(),
        // v1 is read-only, so the safe assumption for a calendar whose role Google did not
        // state is the least privileged one.
        access_role: entry
            .access_role
            .clone()
            .unwrap_or_else(|| "reader".to_string()),
        is_primary: entry.primary.unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------------------
// events.list
// ---------------------------------------------------------------------------------------

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, NaiveDate, TimeZone};
use chrono_tz::Tz;

use crate::store::Event;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsPage {
    #[serde(default)]
    pub items: Vec<WireEvent>,
    pub next_page_token: Option<String>,
    /// Present only on the last page, and only when a sync token was requested. This is the
    /// cursor the next incremental sync sends.
    pub next_sync_token: Option<String>,
    /// The calendar's own zone, used for all-day events that carry no zone of their own.
    pub time_zone: Option<String>,
}

/// One event as Google sends it.
///
/// Almost everything is optional because a deleted event arrives as little more than an id
/// and `status: "cancelled"` — a struct that insisted on `start` would fail to parse the
/// very payload that tells us to remove a row.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireEvent {
    pub id: String,
    pub status: Option<String>,
    pub etag: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: Option<WireTime>,
    pub end: Option<WireTime>,
    /// RRULE plus any EXDATE and RDATE lines, as Google splits them.
    #[serde(default)]
    pub recurrence: Vec<String>,
    pub recurring_event_id: Option<String>,
    pub original_start_time: Option<WireTime>,
    #[serde(rename = "iCalUID")]
    pub ical_uid: Option<String>,
    pub updated: Option<String>,
}

/// Google sends `date` for all-day events and `dateTime` for timed ones, never both.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTime {
    pub date: Option<String>,
    pub date_time: Option<String>,
    pub time_zone: Option<String>,
}

/// What one wire event means for the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mapped {
    Store(Box<Event>),
    /// Google's way of saying "this is gone". It arrives with no times and no summary, so
    /// there is nothing to store — only a row to remove.
    Cancelled {
        id: String,
    },
}

impl WireTime {
    /// UTC seconds, and whether this was an all-day value.
    ///
    /// An all-day event has no instant of its own: "15 September" starts at a different
    /// moment in Madrid and in São Paulo. SPEC §4 fixes the convention as midnight in the
    /// event's own zone, falling back to the calendar's when Google states none.
    fn to_utc(&self, fallback: Tz) -> Result<(i64, bool)> {
        if let Some(date_time) = &self.date_time {
            let parsed = DateTime::parse_from_rfc3339(date_time)
                .with_context(|| format!("could not read the timestamp {date_time}"))?;
            return Ok((parsed.timestamp(), false));
        }

        let date = self
            .date
            .as_ref()
            .ok_or_else(|| anyhow!("a time with neither date nor dateTime"))?;
        let naive = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .with_context(|| format!("could not read the date {date}"))?;
        let zone = self
            .time_zone
            .as_deref()
            .and_then(|name| name.parse::<Tz>().ok())
            .unwrap_or(fallback);
        let midnight = naive
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("{date} has no midnight"))?;
        let instant = zone
            .from_local_datetime(&midnight)
            .earliest()
            // A DST spring-forward can delete local midnight outright, as it does in São
            // Paulo. The day still exists; it just starts an hour later.
            .or_else(|| zone.from_local_datetime(&midnight).latest())
            .ok_or_else(|| anyhow!("{date} has no valid start in {zone}"))?;
        Ok((instant.timestamp(), true))
    }
}

/// Google's event as this app's own.
///
/// Pure, so it is testable against recorded fixtures — which SPEC §8 requires, because live
/// credentials may not exist and this is where external data becomes ours.
pub fn map_event(
    account: &str,
    calendar_id: &str,
    calendar_timezone: Tz,
    wire: &WireEvent,
) -> Result<Mapped> {
    if wire.status.as_deref() == Some("cancelled") {
        return Ok(Mapped::Cancelled {
            id: wire.id.clone(),
        });
    }

    let start = wire
        .start
        .as_ref()
        .ok_or_else(|| anyhow!("event {} has no start", wire.id))?
        .to_utc(calendar_timezone)?;
    let end = wire
        .end
        .as_ref()
        .ok_or_else(|| anyhow!("event {} has no end", wire.id))?
        .to_utc(calendar_timezone)?;

    let original_start_utc = wire
        .original_start_time
        .as_ref()
        .map(|time| time.to_utc(calendar_timezone))
        .transpose()?
        .map(|(instant, _)| instant);

    Ok(Mapped::Store(Box::new(Event {
        account: account.to_string(),
        calendar_id: calendar_id.to_string(),
        id: wire.id.clone(),
        ical_uid: wire.ical_uid.clone(),
        etag: wire.etag.clone(),
        summary: wire.summary.clone().unwrap_or_default(),
        description: wire.description.clone(),
        location: wire.location.clone(),
        start_utc: start.0,
        end_utc: end.0,
        timezone: wire
            .start
            .as_ref()
            .and_then(|time| time.time_zone.clone())
            .or_else(|| Some(calendar_timezone.name().to_string())),
        all_day: start.1,
        // Newline-joined and whole. Keeping only the RRULE would resurrect occurrences the
        // user deleted (EXDATE) and lose ones they added (RDATE).
        rrule: (!wire.recurrence.is_empty()).then(|| wire.recurrence.join("\n")),
        recurring_event_id: wire.recurring_event_id.clone(),
        original_start_utc,
        status: wire
            .status
            .clone()
            .unwrap_or_else(|| "confirmed".to_string()),
        updated_at: wire
            .updated
            .as_deref()
            .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
            .map(|stamp| stamp.timestamp()),
    })))
}

#[cfg(test)]
mod calendar_tests {
    use super::*;

    const PAGE1: &str = include_str!("../../tests/fixtures/calendar_list_page1.json");
    const PAGE2: &str = include_str!("../../tests/fixtures/calendar_list_page2.json");

    fn page1() -> CalendarListPage {
        serde_json::from_str(PAGE1).expect("the recorded calendarList shape must parse")
    }

    #[test]
    fn the_recorded_response_parses_with_every_field_we_store() {
        let page = page1();
        assert_eq!(page.items.len(), 3);
        let primary = &page.items[0];
        assert_eq!(primary.id, "personal@example.com");
        assert_eq!(primary.background_color.as_deref(), Some("#9fe1e7"));
        assert_eq!(primary.time_zone.as_deref(), Some("Europe/Madrid"));
        assert_eq!(primary.access_role.as_deref(), Some("owner"));
        assert_eq!(primary.primary, Some(true));
    }

    #[test]
    fn fields_google_leaves_out_do_not_fail_the_whole_page() {
        // The third entry has no timeZone and no primary. One calendar missing an optional
        // field must not cost the account every other calendar on it.
        let page = page1();
        let team = &page.items[2];
        assert_eq!(team.time_zone, None);
        assert_eq!(team.primary, None);
    }

    #[test]
    fn a_secondary_calendar_is_not_marked_primary() {
        let page = page1();
        let holidays = calendar_metadata("personal@example.com", &page.items[1]);
        assert!(!holidays.is_primary);
        let primary = calendar_metadata("personal@example.com", &page.items[0]);
        assert!(primary.is_primary);
    }

    #[test]
    fn a_calendar_whose_role_google_omitted_is_assumed_read_only() {
        let entry = CalendarListEntry {
            id: "c_x@group.calendar.google.com".to_string(),
            summary: Some("X".to_string()),
            background_color: None,
            time_zone: None,
            access_role: None,
            primary: None,
        };
        assert_eq!(
            calendar_metadata("a@example.com", &entry).access_role,
            "reader"
        );
    }

    #[test]
    fn an_unnamed_calendar_falls_back_to_its_id_rather_than_a_blank_row() {
        let entry = CalendarListEntry {
            id: "c_nameless@group.calendar.google.com".to_string(),
            summary: None,
            background_color: None,
            time_zone: None,
            access_role: Some("reader".to_string()),
            primary: None,
        };
        let mapped = calendar_metadata("a@example.com", &entry);
        assert_eq!(mapped.summary, "c_nameless@group.calendar.google.com");
    }

    #[test]
    fn the_mapping_keys_every_calendar_to_the_account_that_fetched_it() {
        // The same calendar id can be on two accounts; only the account tells them apart.
        let page = page1();
        let personal = calendar_metadata("personal@example.com", &page.items[0]);
        let work = calendar_metadata("work@example.com", &page.items[0]);
        assert_eq!(personal.id, work.id);
        assert_ne!(personal.account, work.account);
    }

    #[test]
    fn a_first_page_carries_a_token_and_a_last_page_does_not() {
        assert_eq!(
            page1().next_page_token.as_deref(),
            Some("CigKGjRkYzM4ZjE2LTY")
        );
        let last: CalendarListPage = serde_json::from_str(PAGE2).unwrap();
        assert_eq!(last.next_page_token, None);
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;

    const PAGE1: &str = include_str!("../../tests/fixtures/events_page1.json");
    const PAGE2: &str = include_str!("../../tests/fixtures/events_page2.json");
    const SHARED: &str = include_str!("../../tests/fixtures/events_shared_invite.json");

    const MADRID: Tz = chrono_tz::Europe::Madrid;
    const SAO_PAULO: Tz = chrono_tz::America::Sao_Paulo;

    fn page1() -> EventsPage {
        serde_json::from_str(PAGE1).expect("the recorded events.list shape must parse")
    }

    fn stored(wire: &WireEvent, zone: Tz) -> Event {
        match map_event("work@example.com", "primary", zone, wire).unwrap() {
            Mapped::Store(event) => *event,
            Mapped::Cancelled { id } => panic!("{id} was mapped as cancelled"),
        }
    }

    fn by_id<'a>(page: &'a EventsPage, id: &str) -> &'a WireEvent {
        page.items
            .iter()
            .find(|event| event.id == id)
            .unwrap_or_else(|| panic!("the fixture has no event {id}"))
    }

    #[test]
    fn a_timed_event_becomes_the_instant_it_actually_happens() {
        let page = page1();
        let event = stored(by_id(&page, "timed001"), MADRID);
        assert_eq!(event.start_utc, 1_789_371_000);
        assert_eq!(event.end_utc, 1_789_371_900);
        assert!(!event.all_day);
    }

    #[test]
    fn an_all_day_event_starts_at_midnight_in_its_own_zone() {
        // SPEC §4 fixes the convention, because "15 September" is not an instant.
        let page = page1();
        let event = stored(by_id(&page, "allday001"), MADRID);
        assert!(event.all_day);
        assert_eq!(event.start_utc, 1_789_423_200, "midnight in Madrid");
    }

    #[test]
    fn the_same_all_day_date_is_a_different_instant_in_another_zone() {
        // The whole reason all-day events cannot be stored as a bare date. Getting this
        // wrong puts a holiday on the wrong day for one of the user's accounts.
        let page = page1();
        let madrid = stored(by_id(&page, "allday001"), MADRID);
        let sao_paulo = stored(by_id(&page, "allday001"), SAO_PAULO);
        assert_eq!(sao_paulo.start_utc, 1_789_441_200);
        assert_ne!(madrid.start_utc, sao_paulo.start_utc);
    }

    #[test]
    fn an_all_day_event_falls_back_to_the_calendars_zone_when_google_states_none() {
        let page = page1();
        let allday = by_id(&page, "allday001");
        assert!(
            allday.start.as_ref().unwrap().time_zone.is_none(),
            "the fixture must exercise the fallback, or this proves nothing"
        );
        assert_eq!(stored(allday, MADRID).start_utc, 1_789_423_200);
    }

    #[test]
    fn an_event_spanning_midnight_keeps_both_ends() {
        let page = page1();
        let event = stored(by_id(&page, "overnight001"), MADRID);
        assert_eq!(event.start_utc, 1_789_588_800);
        assert_eq!(event.end_utc, 1_789_617_600);
        assert!(event.end_utc > event.start_utc);
    }

    #[test]
    fn the_whole_recurrence_block_is_kept_not_only_the_rrule() {
        // Dropping EXDATE resurrects occurrences the user deleted; dropping RDATE loses
        // ones they added. Both are silent, and both look like a sync bug months later.
        let page = page1();
        let event = stored(by_id(&page, "master001"), MADRID);
        let rrule = event
            .rrule
            .expect("a recurring master must carry its recurrence");
        assert!(rrule.contains("RRULE:FREQ=WEEKLY"));
        assert!(rrule.contains("EXDATE"), "EXDATE must survive the mapping");
        assert!(rrule.contains("RDATE"), "RDATE must survive the mapping");
        assert_eq!(rrule.lines().count(), 3);
    }

    #[test]
    fn a_modified_instance_records_the_occurrence_it_replaces() {
        let page = page1();
        let event = stored(by_id(&page, "master001_20260921T140000Z"), MADRID);
        assert_eq!(event.recurring_event_id.as_deref(), Some("master001"));
        assert_eq!(
            event.original_start_utc,
            Some(1_789_999_200),
            "without the original start, expansion cannot tell which occurrence this replaces"
        );
        assert_eq!(event.start_utc, 1_790_004_600, "and it has moved");
    }

    #[test]
    fn a_cancelled_event_is_a_deletion_not_a_parse_failure() {
        // A deleted event arrives with little more than an id. A mapping that insisted on
        // `start` would fail on the very payload that says to remove the row.
        let page = page1();
        let mapped = map_event(
            "work@example.com",
            "primary",
            MADRID,
            by_id(&page, "deleted001"),
        )
        .unwrap();
        assert_eq!(
            mapped,
            Mapped::Cancelled {
                id: "deleted001".to_string()
            }
        );
    }

    #[test]
    fn every_stored_event_carries_its_ical_uid() {
        // Nothing in v1 reads it, so nothing else would catch it silently going null — and
        // the cost of finding out later is a full resync of every calendar (SPEC §8).
        let page = page1();
        for wire in &page.items {
            if let Mapped::Store(event) =
                map_event("a@example.com", "primary", MADRID, wire).unwrap()
            {
                assert!(
                    event.ical_uid.is_some(),
                    "event {} lost its ical_uid",
                    event.id
                );
            }
        }
    }

    #[test]
    fn the_same_meeting_on_two_accounts_maps_to_one_ical_uid_and_two_rows() {
        // SPEC §8's named case, and what makes the duplicate preference in §10 buildable
        // later without a resync.
        let page: EventsPage = serde_json::from_str(SHARED).unwrap();
        let personal = stored(&page.items[0], MADRID);
        let work = stored(&page.items[1], MADRID);

        assert_ne!(personal.id, work.id, "two genuinely separate rows");
        assert_eq!(personal.ical_uid, work.ical_uid);
        assert_eq!(
            personal.ical_uid.as_deref(),
            Some("quarterly-planning-2026q4@google.com")
        );
    }

    #[test]
    fn a_recurring_series_shares_one_ical_uid_across_master_and_instance() {
        // Which is why ical_uid is not a unique key: any dedup built on it must group by
        // (ical_uid, start_utc) at minimum.
        let page = page1();
        let master = stored(by_id(&page, "master001"), MADRID);
        let instance = stored(by_id(&page, "master001_20260921T140000Z"), MADRID);
        assert_eq!(master.ical_uid, instance.ical_uid);
        assert_ne!(master.id, instance.id);
    }

    #[test]
    fn the_mapping_keys_every_event_to_the_account_and_calendar_that_fetched_it() {
        let page = page1();
        let wire = by_id(&page, "timed001");
        let personal = map_event("personal@example.com", "primary", MADRID, wire).unwrap();
        let work = map_event("work@example.com", "team", MADRID, wire).unwrap();
        let (Mapped::Store(personal), Mapped::Store(work)) = (personal, work) else {
            panic!("both should store");
        };
        assert_eq!(personal.id, work.id);
        assert_ne!(personal.account, work.account);
        assert_ne!(personal.calendar_id, work.calendar_id);
    }

    #[test]
    fn a_first_page_carries_a_page_token_and_the_last_carries_a_sync_token() {
        let first = page1();
        assert_eq!(first.next_page_token.as_deref(), Some("CjgKGjRkYzM4ZjE2"));
        assert_eq!(first.next_sync_token, None);

        let last: EventsPage = serde_json::from_str(PAGE2).unwrap();
        assert_eq!(last.next_page_token, None);
        assert_eq!(
            last.next_sync_token.as_deref(),
            Some("CJDFnbHF7YkDEJDFnbHF7YkDGAUg"),
            "this is the cursor the next incremental sync sends"
        );
    }

    #[test]
    fn the_updated_stamp_becomes_unix_seconds() {
        let page = page1();
        let event = stored(by_id(&page, "timed001"), MADRID);
        assert!(event.updated_at.is_some());
    }
}
