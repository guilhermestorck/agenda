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

#[cfg(test)]
mod tests {
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
