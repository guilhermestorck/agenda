//! The agenda list: events in the order they happen, under a heading per day.
//!
//! The grouping is pure and lives here rather than in the widget, because the rules that
//! matter — which day an event belongs to, and what order events share within one — are
//! exactly the ones worth testing.

use chrono::{DateTime, NaiveDate, TimeZone};
use chrono_tz::Tz;

use super::week::Item;

/// One day's events, in the order they should be read.
#[derive(Debug, Clone)]
pub struct DayGroup {
    pub date: NaiveDate,
    pub items: Vec<Item>,
}

/// The day an event belongs under: the day its start falls in, **in the display zone**.
///
/// Not UTC. An event at 00:30 Madrid is on the 22nd for the user and the 21st in UTC, and
/// filing it under the 21st would put it beneath yesterday's heading.
fn day_of(start_utc: i64, zone: Tz) -> Option<NaiveDate> {
    let instant: DateTime<Tz> = zone.timestamp_opt(start_utc, 0).single()?;
    Some(instant.date_naive())
}

/// Group events under one heading per day, each day's events in reading order.
///
/// All-day events come first within a day: they apply to the whole of it, so placing them
/// among the timed events would imply a time they do not have. Remaining ties break on
/// summary so the order does not shuffle between redraws of the same data.
pub fn group(items: &[Item], zone: Tz) -> Vec<DayGroup> {
    let mut days: std::collections::BTreeMap<NaiveDate, Vec<Item>> = Default::default();
    for item in items {
        let Some(date) = day_of(item.start_utc, zone) else {
            continue;
        };
        days.entry(date).or_default().push(item.clone());
    }

    days.into_iter()
        .map(|(date, mut items)| {
            items.sort_by(|a, b| {
                b.all_day
                    .cmp(&a.all_day)
                    .then(a.start_utc.cmp(&b.start_utc))
                    .then_with(|| a.summary.cmp(&b.summary))
            });
            DayGroup { date, items }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::style::Colors;

    const MADRID: Tz = chrono_tz::Europe::Madrid;

    fn at(local: &str) -> i64 {
        let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").unwrap();
        MADRID
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn item(summary: &str, local: &str, minutes: i64, all_day: bool) -> Item {
        Item {
            summary: summary.to_string(),
            start_utc: at(local),
            end_utc: at(local) + minutes * 60,
            all_day,
            colors: Colors {
                fill: "#3584e4".to_string(),
                marker: "#3584e4".to_string(),
            },
            account: "work@example.com".to_string(),
            picture: None,
        }
    }

    fn on(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn summaries(group: &DayGroup) -> Vec<&str> {
        group
            .items
            .iter()
            .map(|item| item.summary.as_str())
            .collect()
    }

    #[test]
    fn days_come_out_in_order_with_one_group_each() {
        let items = vec![
            item("later", "2026-09-16 09:00:00", 30, false),
            item("earlier", "2026-09-14 09:00:00", 30, false),
            item("same day", "2026-09-14 15:00:00", 30, false),
        ];
        let groups = group(&items, MADRID);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].date, on("2026-09-14"));
        assert_eq!(groups[1].date, on("2026-09-16"));
        assert_eq!(summaries(&groups[0]), vec!["earlier", "same day"]);
    }

    #[test]
    fn all_day_events_lead_their_day() {
        // An all-day event applies to the whole day; slotting it among the timed ones by
        // its midnight start implies a time it does not have.
        // The all-day event must lead even when a timed event starts earlier in the day.
        // Google returns all-day events as dates, and once converted their instant can land
        // after midnight local — so sorting on start_utc alone puts the night shift above
        // the holiday it falls on.
        let items = vec![
            item("night shift", "2026-09-14 00:30:00", 60, false),
            item("public holiday", "2026-09-14 02:00:00", 1440, true),
            item("standup", "2026-09-14 09:00:00", 15, false),
        ];
        let groups = group(&items, MADRID);
        assert_eq!(
            summaries(&groups[0]),
            vec!["public holiday", "night shift", "standup"]
        );
    }

    #[test]
    fn events_at_the_same_instant_keep_a_stable_order() {
        // Without the summary tie-break these two could swap between redraws of identical
        // data, which reads as the list flickering.
        let items = vec![
            item("zebra", "2026-09-14 09:00:00", 30, false),
            item("alpha", "2026-09-14 09:00:00", 30, false),
        ];
        let first = group(&items, MADRID);
        let reversed: Vec<Item> = items.into_iter().rev().collect();
        let second = group(&reversed, MADRID);
        assert_eq!(summaries(&first[0]), vec!["alpha", "zebra"]);
        assert_eq!(summaries(&first[0]), summaries(&second[0]));
    }

    #[test]
    fn an_event_just_after_midnight_files_under_the_local_day() {
        // 00:30 in Madrid is the previous day in UTC. Filing by UTC would put it under
        // yesterday's heading, below events that have already happened.
        let items = vec![item("night owl", "2026-09-15 00:30:00", 30, false)];
        let groups = group(&items, MADRID);
        assert_eq!(groups[0].date, on("2026-09-15"));
    }

    #[test]
    fn an_empty_list_produces_no_groups() {
        assert!(group(&[], MADRID).is_empty());
    }
}
