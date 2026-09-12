//! Turning a stored recurring master into the occurrences a week actually contains.
//!
//! This is where calendar applications break, so the rules are stated rather than implied:
//!
//! - Google stores the recurrence *without* a `DTSTART`; the master's own start supplies it.
//!   Rebuilding that line in the event's own zone is what keeps a 09:00 meeting at 09:00
//!   after a DST transition instead of drifting by an hour.
//! - An occurrence's length is taken from the master as a duration, so an end time follows
//!   its start across a transition rather than being computed independently.
//! - All-day events have no instant of their own and are expanded as dates.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::{Tz, UTC};

use crate::store::Event;

mod window;
pub use window::occurrences_in_window;

/// A single concrete occurrence of an event, resolved to instants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub event: Event,
    pub start_utc: i64,
    pub end_utc: i64,
}

impl Occurrence {
    /// Whether this occurrence overlaps `[start, end)`. Overlap, not containment: something
    /// that began yesterday and ends today is on today's grid.
    pub fn overlaps(&self, start_utc: i64, end_utc: i64) -> bool {
        self.start_utc < end_utc && self.end_utc > start_utc
    }
}

/// A ceiling on how many occurrences one series may contribute, so an unbounded or
/// pathological rule cannot hang the UI thread that asked for a week.
///
/// ponytail: a flat cap rather than a cleverer guard. A week holds 168 hours; a series
/// producing more than this in one window is already unusable as a calendar.
const MAX_OCCURRENCES: u16 = 1_000;

fn zone_of(event: &Event) -> Tz {
    event
        .timezone
        .as_deref()
        .and_then(|name| name.parse::<Tz>().ok())
        .unwrap_or(UTC)
}

fn instant(utc_seconds: i64) -> Result<DateTime<Utc>> {
    Utc.timestamp_opt(utc_seconds, 0)
        .single()
        .context("an event's stored start is not a real instant")
}

/// Rebuild the `DTSTART` line Google leaves out.
///
/// The zone matters: `DTSTART;TZID=Europe/Madrid:20260914T090000` with `FREQ=DAILY` means
/// 09:00 Madrid every day, transitions included. The same series pinned to a UTC offset
/// would silently become 08:00 for half the year.
fn rule_text(event: &Event, recurrence: &str) -> Result<String> {
    let zone = zone_of(event);
    let start = instant(event.start_utc)?.with_timezone(&zone);
    let dtstart = if event.all_day {
        format!("DTSTART;VALUE=DATE:{}", start.format("%Y%m%d"))
    } else {
        format!(
            "DTSTART;TZID={}:{}",
            zone.name(),
            start.format("%Y%m%dT%H%M%S")
        )
    };
    Ok(format!("{dtstart}\n{recurrence}"))
}

/// Every occurrence of `event` overlapping `[window_start, window_end)`.
///
/// A non-recurring event is its own single occurrence. A master expands; its own stored row
/// is never an occurrence in its own right beyond the first the rule produces.
pub fn expand(event: &Event, window_start: i64, window_end: i64) -> Result<Vec<Occurrence>> {
    let duration = event.end_utc - event.start_utc;

    let Some(recurrence) = event.rrule.as_deref() else {
        let single = Occurrence {
            event: event.clone(),
            start_utc: event.start_utc,
            end_utc: event.end_utc,
        };
        return Ok(if single.overlaps(window_start, window_end) {
            vec![single]
        } else {
            Vec::new()
        });
    };

    let text = rule_text(event, recurrence)?;
    let set: rrule::RRuleSet = text
        .parse()
        .with_context(|| format!("could not read the recurrence of {}", event.id))?;

    // Widened by the event's own length so a series occurrence that began before the window
    // and runs into it is still produced — the overlap filter then decides.
    let after = instant(window_start - duration.max(0))?.with_timezone(&rrule::Tz::UTC);
    let before = instant(window_end)?.with_timezone(&rrule::Tz::UTC);

    let result = set.after(after).before(before).all(MAX_OCCURRENCES);
    if result.limited {
        tracing::warn!(
            event = %event.id,
            "the recurrence produced more occurrences than one window can hold; truncated"
        );
    }

    Ok(result
        .dates
        .into_iter()
        .map(|start| {
            let start_utc = start.timestamp();
            Occurrence {
                event: event.clone(),
                start_utc,
                end_utc: start_utc + duration,
            }
        })
        .filter(|occurrence| occurrence.overlaps(window_start, window_end))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    const MADRID: Tz = chrono_tz::Europe::Madrid;
    const SAO_PAULO: Tz = chrono_tz::America::Sao_Paulo;

    /// A master starting at the given local time in the given zone.
    fn master(zone: Tz, local: &str, minutes: i64, recurrence: Option<&str>) -> Event {
        let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S")
            .expect("a test timestamp");
        let start = zone
            .from_local_datetime(&naive)
            .single()
            .expect("an unambiguous test timestamp");
        Event {
            account: "work@example.com".to_string(),
            calendar_id: "primary".to_string(),
            id: "series".to_string(),
            ical_uid: Some("series@google.com".to_string()),
            etag: None,
            summary: "Series".to_string(),
            description: None,
            location: None,
            start_utc: start.timestamp(),
            end_utc: start.timestamp() + minutes * 60,
            timezone: Some(zone.name().to_string()),
            all_day: false,
            rrule: recurrence.map(str::to_string),
            recurring_event_id: None,
            original_start_utc: None,
            status: "confirmed".to_string(),
            reminder_minutes: None,
            updated_at: None,
        }
    }

    fn window(zone: Tz, from: &str, to: &str) -> (i64, i64) {
        let parse = |text: &str| {
            let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap();
            zone.from_local_datetime(&naive)
                .earliest()
                .unwrap()
                .timestamp()
        };
        (parse(from), parse(to))
    }

    /// Local wall-clock times of each occurrence, which is what a user actually sees.
    fn local_times(occurrences: &[Occurrence], zone: Tz) -> Vec<String> {
        occurrences
            .iter()
            .map(|occurrence| {
                Utc.timestamp_opt(occurrence.start_utc, 0)
                    .unwrap()
                    .with_timezone(&zone)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn a_non_recurring_event_is_its_own_single_occurrence() {
        let event = master(MADRID, "2026-09-14 09:30:00", 15, None);
        let (from, to) = window(MADRID, "2026-09-14 00:00:00", "2026-09-21 00:00:00");
        assert_eq!(expand(&event, from, to).unwrap().len(), 1);
    }

    #[test]
    fn a_non_recurring_event_outside_the_window_produces_nothing() {
        let event = master(MADRID, "2026-08-14 09:30:00", 15, None);
        let (from, to) = window(MADRID, "2026-09-14 00:00:00", "2026-09-21 00:00:00");
        assert!(expand(&event, from, to).unwrap().is_empty());
    }

    #[test]
    fn recurring_masters_survive_the_range_filter() {
        // The master starts months before the window. A filter on the master's own start
        // would show an empty week for nearly every real calendar.
        let event = master(
            MADRID,
            "2026-01-05 16:00:00",
            60,
            Some("RRULE:FREQ=WEEKLY;BYDAY=MO"),
        );
        let (from, to) = window(MADRID, "2026-09-14 00:00:00", "2026-09-21 00:00:00");
        let occurrences = expand(&event, from, to).unwrap();
        assert_eq!(local_times(&occurrences, MADRID), vec!["2026-09-14 16:00"]);
    }

    #[test]
    fn an_exdate_removes_the_occurrence_it_names() {
        // Dropping EXDATE resurrects an occurrence the user deliberately deleted.
        let event = master(
            MADRID,
            "2026-09-07 16:00:00",
            60,
            Some("RRULE:FREQ=WEEKLY;BYDAY=MO\nEXDATE;TZID=Europe/Madrid:20260914T160000"),
        );
        let (from, to) = window(MADRID, "2026-09-07 00:00:00", "2026-09-28 00:00:00");
        let times = local_times(&expand(&event, from, to).unwrap(), MADRID);
        assert!(
            !times.contains(&"2026-09-14 16:00".to_string()),
            "got {times:?}"
        );
        assert!(
            times.contains(&"2026-09-21 16:00".to_string()),
            "got {times:?}"
        );
    }

    #[test]
    fn an_rdate_adds_an_occurrence_the_rule_would_not_produce() {
        // Dropping RDATE loses an occurrence the user deliberately added.
        let event = master(
            MADRID,
            "2026-09-07 16:00:00",
            60,
            Some("RRULE:FREQ=WEEKLY;BYDAY=MO\nRDATE;TZID=Europe/Madrid:20260916T160000"),
        );
        let (from, to) = window(MADRID, "2026-09-14 00:00:00", "2026-09-21 00:00:00");
        let times = local_times(&expand(&event, from, to).unwrap(), MADRID);
        assert!(
            times.contains(&"2026-09-16 16:00".to_string()),
            "the added Wednesday must appear; got {times:?}"
        );
    }

    #[test]
    fn an_occurrence_that_began_before_the_window_and_runs_into_it_is_produced() {
        let event = master(
            MADRID,
            "2026-09-13 22:00:00",
            8 * 60,
            Some("RRULE:FREQ=DAILY"),
        );
        // The window opens at midnight; the night shift started two hours earlier.
        let (from, to) = window(MADRID, "2026-09-14 00:00:00", "2026-09-14 12:00:00");
        let occurrences = expand(&event, from, to).unwrap();
        assert!(
            occurrences
                .iter()
                .any(|occurrence| occurrence.start_utc < from && occurrence.end_utc > from),
            "an event spanning midnight belongs to both days"
        );
    }

    #[test]
    fn a_daily_series_keeps_its_local_time_across_the_madrid_spring_forward() {
        // 29 March 2026: 02:00 becomes 03:00. A series pinned to an offset rather than a
        // zone silently becomes 08:00 for half the year.
        let event = master(MADRID, "2026-03-27 09:00:00", 60, Some("RRULE:FREQ=DAILY"));
        let (from, to) = window(MADRID, "2026-03-27 00:00:00", "2026-03-31 00:00:00");
        let times = local_times(&expand(&event, from, to).unwrap(), MADRID);
        assert_eq!(
            times,
            vec![
                "2026-03-27 09:00",
                "2026-03-28 09:00",
                "2026-03-29 09:00",
                "2026-03-30 09:00",
            ]
        );
    }

    #[test]
    fn a_daily_series_keeps_its_local_time_across_the_madrid_autumn_back() {
        // 25 October 2026: 03:00 becomes 02:00.
        let event = master(MADRID, "2026-10-23 09:00:00", 60, Some("RRULE:FREQ=DAILY"));
        let (from, to) = window(MADRID, "2026-10-23 00:00:00", "2026-10-27 00:00:00");
        let times = local_times(&expand(&event, from, to).unwrap(), MADRID);
        assert_eq!(
            times,
            vec![
                "2026-10-23 09:00",
                "2026-10-24 09:00",
                "2026-10-25 09:00",
                "2026-10-26 09:00",
            ]
        );
    }

    #[test]
    fn the_utc_offset_actually_changes_across_the_madrid_transition() {
        // Without this, the two tests above would pass just as happily against a zone with
        // no transition at all, and prove nothing.
        let event = master(MADRID, "2026-03-27 09:00:00", 60, Some("RRULE:FREQ=DAILY"));
        let (from, to) = window(MADRID, "2026-03-27 00:00:00", "2026-03-31 00:00:00");
        let occurrences = expand(&event, from, to).unwrap();
        let gap_before = occurrences[1].start_utc - occurrences[0].start_utc;
        let gap_across = occurrences[2].start_utc - occurrences[1].start_utc;
        assert_eq!(gap_before, 86_400);
        assert_eq!(
            gap_across, 82_800,
            "the day containing the transition is 23 hours long"
        );
    }

    #[test]
    fn a_daily_series_keeps_its_local_time_across_the_sao_paulo_spring_forward() {
        // Brazil abolished DST in 2019, so the only real transitions in this zone are
        // historical. 4 November 2018: midnight becomes 01:00.
        let event = master(
            SAO_PAULO,
            "2018-11-02 09:00:00",
            60,
            Some("RRULE:FREQ=DAILY"),
        );
        let (from, to) = window(SAO_PAULO, "2018-11-02 00:00:00", "2018-11-06 00:00:00");
        let times = local_times(&expand(&event, from, to).unwrap(), SAO_PAULO);
        assert_eq!(
            times,
            vec![
                "2018-11-02 09:00",
                "2018-11-03 09:00",
                "2018-11-04 09:00",
                "2018-11-05 09:00",
            ]
        );
    }

    #[test]
    fn a_daily_series_keeps_its_local_time_across_the_sao_paulo_autumn_back() {
        // 17 February 2019: the last DST transition Brazil ever had.
        let event = master(
            SAO_PAULO,
            "2019-02-15 09:00:00",
            60,
            Some("RRULE:FREQ=DAILY"),
        );
        let (from, to) = window(SAO_PAULO, "2019-02-15 00:00:00", "2019-02-19 00:00:00");
        let times = local_times(&expand(&event, from, to).unwrap(), SAO_PAULO);
        assert_eq!(
            times,
            vec![
                "2019-02-15 09:00",
                "2019-02-16 09:00",
                "2019-02-17 09:00",
                "2019-02-18 09:00",
            ]
        );
    }

    #[test]
    fn the_utc_offset_actually_changes_across_the_sao_paulo_transition() {
        let event = master(
            SAO_PAULO,
            "2018-11-02 09:00:00",
            60,
            Some("RRULE:FREQ=DAILY"),
        );
        let (from, to) = window(SAO_PAULO, "2018-11-02 00:00:00", "2018-11-06 00:00:00");
        let occurrences = expand(&event, from, to).unwrap();
        let gap_across = occurrences[2].start_utc - occurrences[1].start_utc;
        assert_eq!(gap_across, 82_800, "the transition day is 23 hours long");
    }

    #[test]
    fn an_occurrences_end_follows_its_start_across_a_transition() {
        let event = master(MADRID, "2026-03-27 09:00:00", 60, Some("RRULE:FREQ=DAILY"));
        let (from, to) = window(MADRID, "2026-03-27 00:00:00", "2026-03-31 00:00:00");
        for occurrence in expand(&event, from, to).unwrap() {
            let end = Utc
                .timestamp_opt(occurrence.end_utc, 0)
                .unwrap()
                .with_timezone(&MADRID);
            assert_eq!(end.hour(), 10, "a one-hour meeting stays one hour long");
        }
    }

    #[test]
    fn an_all_day_series_lands_on_the_right_day_in_its_own_zone() {
        let mut event = master(
            MADRID,
            "2026-09-07 00:00:00",
            24 * 60,
            Some("RRULE:FREQ=WEEKLY;BYDAY=MO"),
        );
        event.all_day = true;
        let (from, to) = window(MADRID, "2026-09-14 00:00:00", "2026-09-21 00:00:00");
        let occurrences = expand(&event, from, to).unwrap();
        assert_eq!(occurrences.len(), 1);
        let day = Utc
            .timestamp_opt(occurrences[0].start_utc, 0)
            .unwrap()
            .with_timezone(&MADRID);
        assert_eq!(
            (day.year(), day.month(), day.day()),
            (2026, 9, 14),
            "an all-day event on the wrong day is the classic timezone bug"
        );
    }

    #[test]
    fn a_malformed_recurrence_is_an_error_naming_the_event_not_a_panic() {
        let event = master(
            MADRID,
            "2026-09-14 09:00:00",
            60,
            Some("RRULE:FREQ=NONSENSE"),
        );
        let (from, to) = window(MADRID, "2026-09-14 00:00:00", "2026-09-21 00:00:00");
        let error = expand(&event, from, to).unwrap_err();
        assert!(error.to_string().contains("series"), "got: {error}");
    }
}
