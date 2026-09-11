//! The one query the week view makes.
//!
//! Everything a window contains, from every connected account at once, with recurring series
//! expanded and their exceptions applied. SPEC §1: there is no current account, so this never
//! filters by one — only by the visibility the user set per calendar.

use std::collections::HashSet;

use anyhow::Result;

use crate::store::{Event, Store};

use super::{Occurrence, expand};

/// Identifies the occurrence an override or tombstone refers to.
type OccurrenceKey = (String, String, String, i64);

fn key_of_override(event: &Event) -> Option<OccurrenceKey> {
    Some((
        event.account.clone(),
        event.calendar_id.clone(),
        event.recurring_event_id.clone()?,
        event.original_start_utc?,
    ))
}

/// Every occurrence overlapping `[start_utc, end_utc)`, sorted by start.
pub fn occurrences_in_window(
    store: &Store,
    start_utc: i64,
    end_utc: i64,
) -> Result<Vec<Occurrence>> {
    let candidates = store.events_for_window(start_utc, end_utc)?;

    let (exceptions, series): (Vec<Event>, Vec<Event>) = candidates
        .into_iter()
        .partition(|event| event.recurring_event_id.is_some());

    // Both a moved occurrence and a deleted one suppress what the rule would have produced;
    // they differ only in whether anything takes its place.
    let replaced: HashSet<OccurrenceKey> = exceptions.iter().filter_map(key_of_override).collect();

    let mut occurrences = Vec::new();
    for event in &series {
        for occurrence in expand(event, start_utc, end_utc)? {
            let key = (
                event.account.clone(),
                event.calendar_id.clone(),
                event.id.clone(),
                occurrence.start_utc,
            );
            if replaced.contains(&key) {
                continue;
            }
            occurrences.push(occurrence);
        }
    }

    for exception in exceptions {
        // A tombstone has already done its work by suppressing the occurrence above.
        if exception.status == "cancelled" {
            continue;
        }
        let occurrence = Occurrence {
            start_utc: exception.start_utc,
            end_utc: exception.end_utc,
            event: exception,
        };
        if occurrence.overlaps(start_utc, end_utc) {
            occurrences.push(occurrence);
        }
    }

    occurrences.sort_by_key(|occurrence| (occurrence.start_utc, occurrence.event.id.clone()));
    Ok(occurrences)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Tz;

    use crate::store::CalendarMetadata;

    const MADRID: Tz = chrono_tz::Europe::Madrid;

    fn at(local: &str) -> i64 {
        let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").unwrap();
        MADRID
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn event(account: &str, calendar: &str, id: &str, local: &str, minutes: i64) -> Event {
        Event {
            account: account.to_string(),
            calendar_id: calendar.to_string(),
            id: id.to_string(),
            ical_uid: Some(format!("{id}@google.com")),
            etag: None,
            summary: id.to_string(),
            description: None,
            location: None,
            start_utc: at(local),
            end_utc: at(local) + minutes * 60,
            timezone: Some("Europe/Madrid".to_string()),
            all_day: false,
            rrule: None,
            recurring_event_id: None,
            original_start_utc: None,
            status: "confirmed".to_string(),
            updated_at: None,
        }
    }

    /// Two accounts, one holding two calendars (SPEC §8).
    fn store() -> Store {
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
        store
    }

    fn week(store: &Store) -> Vec<Occurrence> {
        occurrences_in_window(store, at("2026-09-14 00:00:00"), at("2026-09-21 00:00:00")).unwrap()
    }

    fn ids(occurrences: &[Occurrence]) -> Vec<String> {
        occurrences
            .iter()
            .map(|occurrence| occurrence.event.id.clone())
            .collect()
    }

    #[test]
    fn a_plain_event_passes_through_unchanged() {
        let store = store();
        store
            .upsert_event(&event(
                "work@example.com",
                "primary",
                "dentist",
                "2026-09-15 10:00:00",
                30,
            ))
            .unwrap();
        let found = week(&store);
        assert_eq!(ids(&found), vec!["dentist"]);
        assert_eq!(found[0].start_utc, at("2026-09-15 10:00:00"));
    }

    #[test]
    fn a_series_whose_master_predates_the_window_still_fills_it() {
        let store = store();
        let mut master = event(
            "work@example.com",
            "primary",
            "standup",
            "2026-01-05 09:30:00",
            15,
        );
        master.rrule = Some("RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".to_string());
        store.upsert_event(&master).unwrap();
        assert_eq!(week(&store).len(), 5, "five weekdays in the window");
    }

    #[test]
    fn a_modified_instance_replaces_the_occurrence_it_points_at() {
        let store = store();
        let mut master = event(
            "work@example.com",
            "primary",
            "review",
            "2026-09-07 16:00:00",
            60,
        );
        master.rrule = Some("RRULE:FREQ=WEEKLY;BYDAY=MO".to_string());
        store.upsert_event(&master).unwrap();

        let mut moved = event(
            "work@example.com",
            "primary",
            "review_moved",
            "2026-09-14 17:30:00",
            60,
        );
        moved.recurring_event_id = Some("review".to_string());
        moved.original_start_utc = Some(at("2026-09-14 16:00:00"));
        store.upsert_event(&moved).unwrap();

        let found = week(&store);
        assert_eq!(ids(&found), vec!["review_moved"], "one occurrence, not two");
        assert_eq!(
            found[0].start_utc,
            at("2026-09-14 17:30:00"),
            "and at the time it moved to"
        );
    }

    #[test]
    fn a_cancelled_instance_removes_exactly_that_occurrence() {
        let store = store();
        let mut master = event(
            "work@example.com",
            "primary",
            "review",
            "2026-09-07 16:00:00",
            60,
        );
        master.rrule = Some("RRULE:FREQ=WEEKLY;BYDAY=MO".to_string());
        store.upsert_event(&master).unwrap();

        let mut tombstone = event(
            "work@example.com",
            "primary",
            "review_cancelled",
            "2026-09-14 16:00:00",
            0,
        );
        tombstone.recurring_event_id = Some("review".to_string());
        tombstone.original_start_utc = Some(at("2026-09-14 16:00:00"));
        tombstone.status = "cancelled".to_string();
        store.upsert_event(&tombstone).unwrap();

        assert!(
            week(&store).is_empty(),
            "the only occurrence that week was cancelled"
        );

        let next =
            occurrences_in_window(&store, at("2026-09-21 00:00:00"), at("2026-09-28 00:00:00"))
                .unwrap();
        assert_eq!(next.len(), 1, "and the rest of the series is untouched");
    }

    #[test]
    fn hiding_a_calendar_drops_only_its_events() {
        let store = store();
        store
            .upsert_event(&event(
                "work@example.com",
                "primary",
                "standup",
                "2026-09-15 09:30:00",
                15,
            ))
            .unwrap();
        store
            .upsert_event(&event(
                "work@example.com",
                "team",
                "retro",
                "2026-09-15 15:00:00",
                60,
            ))
            .unwrap();
        assert_eq!(week(&store).len(), 2);

        store
            .set_user_style_for_tests("work@example.com", "team", false, None)
            .unwrap();
        assert_eq!(ids(&week(&store)), vec!["standup"]);
    }

    #[test]
    fn the_window_holds_every_account_at_once() {
        // SPEC §2.3: no switcher, no filter that must be changed to see the rest.
        let store = store();
        store
            .upsert_event(&event(
                "work@example.com",
                "primary",
                "standup",
                "2026-09-15 09:30:00",
                15,
            ))
            .unwrap();
        store
            .upsert_event(&event(
                "personal@example.com",
                "primary",
                "dentist",
                "2026-09-15 10:00:00",
                30,
            ))
            .unwrap();

        let found = week(&store);
        assert_eq!(found.len(), 2);
        let accounts: Vec<&str> = found
            .iter()
            .map(|occurrence| occurrence.event.account.as_str())
            .collect();
        assert!(accounts.contains(&"work@example.com"));
        assert!(accounts.contains(&"personal@example.com"));
    }

    #[test]
    fn every_occurrence_knows_which_account_and_calendar_it_came_from() {
        // The week view needs both to resolve a fill colour and an account marker.
        let store = store();
        store
            .upsert_event(&event(
                "work@example.com",
                "team",
                "retro",
                "2026-09-15 15:00:00",
                60,
            ))
            .unwrap();
        let found = week(&store);
        assert_eq!(found[0].event.account, "work@example.com");
        assert_eq!(found[0].event.calendar_id, "team");
    }

    #[test]
    fn the_same_meeting_on_two_accounts_appears_twice() {
        // SPEC §2.7: duplicates are shown, not merged — each copy carrying its own account.
        let store = store();
        let mut personal = event(
            "personal@example.com",
            "primary",
            "planning_p",
            "2026-09-17 14:00:00",
            60,
        );
        let mut work = event(
            "work@example.com",
            "primary",
            "planning_w",
            "2026-09-17 14:00:00",
            60,
        );
        personal.ical_uid = Some("shared@google.com".to_string());
        work.ical_uid = Some("shared@google.com".to_string());
        store.upsert_event(&personal).unwrap();
        store.upsert_event(&work).unwrap();

        let found = week(&store);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found
                .iter()
                .filter(|o| o.event.ical_uid.as_deref() == Some("shared@google.com"))
                .count(),
            2
        );
    }

    #[test]
    fn an_override_moved_out_of_the_window_takes_its_occurrence_with_it() {
        // The rule would put it on the 14th; the user moved it to the following week. The
        // window must show neither the original nor a ghost.
        let store = store();
        let mut master = event(
            "work@example.com",
            "primary",
            "review",
            "2026-09-07 16:00:00",
            60,
        );
        master.rrule = Some("RRULE:FREQ=WEEKLY;BYDAY=MO".to_string());
        store.upsert_event(&master).unwrap();

        let mut moved = event(
            "work@example.com",
            "primary",
            "review_moved",
            "2026-09-24 16:00:00",
            60,
        );
        moved.recurring_event_id = Some("review".to_string());
        moved.original_start_utc = Some(at("2026-09-14 16:00:00"));
        store.upsert_event(&moved).unwrap();

        assert!(week(&store).is_empty(), "moved away, so the week is empty");
    }

    #[test]
    fn an_override_moved_into_the_window_arrives_with_it() {
        let store = store();
        let mut master = event(
            "work@example.com",
            "primary",
            "review",
            "2026-09-07 16:00:00",
            60,
        );
        master.rrule = Some("RRULE:FREQ=WEEKLY;BYDAY=MO".to_string());
        store.upsert_event(&master).unwrap();

        // The occurrence due on the 28th was pulled forward into this window.
        let mut moved = event(
            "work@example.com",
            "primary",
            "review_moved",
            "2026-09-16 11:00:00",
            60,
        );
        moved.recurring_event_id = Some("review".to_string());
        moved.original_start_utc = Some(at("2026-09-28 16:00:00"));
        store.upsert_event(&moved).unwrap();

        let ids = ids(&week(&store));
        assert!(ids.contains(&"review_moved".to_string()), "got {ids:?}");
        assert!(
            ids.contains(&"review".to_string()),
            "the 14th is still its own occurrence"
        );
    }

    #[test]
    fn occurrences_come_back_in_time_order() {
        let store = store();
        store
            .upsert_event(&event(
                "work@example.com",
                "primary",
                "late",
                "2026-09-18 17:00:00",
                30,
            ))
            .unwrap();
        store
            .upsert_event(&event(
                "personal@example.com",
                "primary",
                "early",
                "2026-09-15 08:00:00",
                30,
            ))
            .unwrap();
        assert_eq!(ids(&week(&store)), vec!["early", "late"]);
    }
}
