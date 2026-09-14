//! Where an occurrence goes on the grid.
//!
//! Kept apart from the widgets because this is the part that can be wrong in ways a
//! screenshot will not show: a meeting an hour off, a night shift missing from the morning
//! it runs into, two overlapping events drawn on top of each other.

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

/// One day's worth of an occurrence. An event spanning midnight produces two.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// 0 = the Monday the week starts on.
    pub day: usize,
    /// Minutes from that day's local midnight.
    pub top_minutes: f64,
    pub height_minutes: f64,
}

/// Minimum drawn height, in minutes, so a five-minute event is still clickable and its title
/// still readable.
const MIN_HEIGHT_MINUTES: f64 = 20.0;

/// Split an occurrence into the per-day pieces the grid draws.
///
/// Clamped to the displayed week, so a series occurrence that started last Sunday
/// contributes only the part that falls inside it.
pub fn segments(
    start_utc: i64,
    end_utc: i64,
    week_start: NaiveDate,
    zone: Tz,
    days: usize,
) -> Vec<Segment> {
    let Some(start) = Utc.timestamp_opt(start_utc, 0).single() else {
        return Vec::new();
    };
    let Some(end) = Utc.timestamp_opt(end_utc.max(start_utc), 0).single() else {
        return Vec::new();
    };
    let start = start.with_timezone(&zone);
    let end = end.with_timezone(&zone);

    let mut out = Vec::new();

    for day in 0..days {
        let date = week_start + Duration::days(day as i64);
        let Some(midnight) = zone
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
        else {
            continue;
        };
        let next_midnight = midnight + Duration::days(1);

        // Overlap with this day, in minutes from its own midnight. Computed against the
        // day's real boundaries rather than a fixed 1440, so a 23- or 25-hour DST day is
        // handled by the same arithmetic as any other.
        let piece_start = start.max(midnight);
        let piece_end = end.min(next_midnight);
        if piece_end < piece_start {
            continue;
        }
        // An event ending exactly at midnight belongs to the day it ran through, not to the
        // next one — but an event with no duration at all still belongs to its own day.
        if piece_end == piece_start && !(start == end && start >= midnight && start < next_midnight)
        {
            continue;
        }

        let top = (piece_start - midnight).num_seconds() as f64 / 60.0;
        let height = (piece_end - piece_start).num_seconds() as f64 / 60.0;
        out.push(Segment {
            day,
            top_minutes: top,
            height_minutes: height.max(MIN_HEIGHT_MINUTES),
        });
    }
    out
}

/// Assign side-by-side columns to overlapping segments within one day.
///
/// Returns, for each input in order, the column it occupies and how many columns that
/// cluster was split into. Greedy interval partitioning: an event takes the leftmost column
/// whose previous occupant has already ended.
pub fn columns(segments: &[Segment]) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by(|a, b| {
        segments[*a]
            .top_minutes
            .partial_cmp(&segments[*b].top_minutes)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut assignment = vec![(0usize, 1usize); segments.len()];
    // Each cluster of mutually overlapping events is widened together, so two events side by
    // side stay the same width as each other.
    let mut cluster: Vec<usize> = Vec::new();
    let mut column_ends: Vec<f64> = Vec::new();
    let mut cluster_end = f64::NEG_INFINITY;

    let finish = |cluster: &mut Vec<usize>,
                  column_ends: &mut Vec<f64>,
                  assignment: &mut Vec<(usize, usize)>| {
        let width = column_ends.len().max(1);
        for index in cluster.iter() {
            assignment[*index].1 = width;
        }
        cluster.clear();
        column_ends.clear();
    };

    for index in order {
        let segment = segments[index];
        if segment.top_minutes >= cluster_end && !cluster.is_empty() {
            finish(&mut cluster, &mut column_ends, &mut assignment);
            cluster_end = f64::NEG_INFINITY;
        }

        let column = column_ends
            .iter()
            .position(|end| *end <= segment.top_minutes)
            .unwrap_or_else(|| {
                column_ends.push(f64::NEG_INFINITY);
                column_ends.len() - 1
            });
        column_ends[column] = segment.top_minutes + segment.height_minutes;
        assignment[index].0 = column;
        cluster.push(index);
        cluster_end = cluster_end.max(segment.top_minutes + segment.height_minutes);
    }
    finish(&mut cluster, &mut column_ends, &mut assignment);
    assignment
}

#[cfg(test)]
mod tests {
    use super::*;

    const MADRID: Tz = chrono_tz::Europe::Madrid;

    fn at(local: &str) -> i64 {
        let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").unwrap();
        MADRID
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp()
    }

    /// SPEC-views-timegrid criterion 4: the same event must land at the same clock position
    /// whether the grid shows one day or seven. Placement follows the date, never the column
    /// count — a regression here would move every event when the user switches span.
    #[test]
    fn an_event_lands_identically_whatever_the_span() {
        let start = at("2026-09-16 09:30:00");
        let end = at("2026-09-16 11:00:00");

        let mut seen = Vec::new();
        for days in [3usize, 5, 7] {
            let found = segments(start, end, week(), MADRID, days);
            assert_eq!(found.len(), 1, "days={days}");
            seen.push((found[0].top_minutes, found[0].height_minutes, found[0].day));
        }
        assert!(
            seen.windows(2).all(|pair| pair[0] == pair[1]),
            "placement drifted with the column count: {seen:?}"
        );

        // A span of one anchored on that day puts it in column zero at the same height.
        let focused = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
        let day_span = segments(start, end, focused, MADRID, 1);
        assert_eq!(day_span.len(), 1);
        assert_eq!(day_span[0].day, 0);
        assert_eq!(day_span[0].top_minutes, seen[0].0);
        assert_eq!(day_span[0].height_minutes, seen[0].1);
    }

    fn week() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
    }

    fn segment(top: f64, height: f64) -> Segment {
        Segment {
            day: 0,
            top_minutes: top,
            height_minutes: height,
        }
    }

    #[test]
    fn a_morning_meeting_lands_on_its_own_day_at_its_own_time() {
        let found = segments(
            at("2026-09-15 09:30:00"),
            at("2026-09-15 10:30:00"),
            week(),
            MADRID,
            7,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].day, 1, "Tuesday is day 1");
        assert_eq!(found[0].top_minutes, 570.0, "09:30 is 570 minutes in");
        assert_eq!(found[0].height_minutes, 60.0);
    }

    #[test]
    fn an_event_spanning_midnight_is_drawn_on_both_days() {
        let found = segments(
            at("2026-09-16 22:00:00"),
            at("2026-09-17 06:00:00"),
            week(),
            MADRID,
            7,
        );
        assert_eq!(found.len(), 2, "a night shift belongs to both days");
        assert_eq!((found[0].day, found[0].top_minutes), (2, 1320.0));
        assert_eq!(found[0].height_minutes, 120.0, "two hours before midnight");
        assert_eq!((found[1].day, found[1].top_minutes), (3, 0.0));
        assert_eq!(found[1].height_minutes, 360.0, "six hours after it");
    }

    #[test]
    fn an_occurrence_starting_before_the_week_contributes_only_its_tail() {
        let found = segments(
            at("2026-09-13 22:00:00"),
            at("2026-09-14 06:00:00"),
            week(),
            MADRID,
            7,
        );
        assert_eq!(found.len(), 1, "Sunday the 13th is not in this week");
        assert_eq!((found[0].day, found[0].top_minutes), (0, 0.0));
        assert_eq!(found[0].height_minutes, 360.0);
    }

    #[test]
    fn an_occurrence_entirely_outside_the_week_draws_nothing() {
        let found = segments(
            at("2026-10-05 09:00:00"),
            at("2026-10-05 10:00:00"),
            week(),
            MADRID,
            7,
        );
        assert!(found.is_empty());
    }

    #[test]
    fn a_very_short_event_is_still_tall_enough_to_read() {
        let found = segments(
            at("2026-09-15 09:00:00"),
            at("2026-09-15 09:05:00"),
            week(),
            MADRID,
            7,
        );
        assert!(found[0].height_minutes >= MIN_HEIGHT_MINUTES);
    }

    #[test]
    fn a_zero_length_event_still_draws_something() {
        let instant = at("2026-09-15 09:00:00");
        let found = segments(instant, instant, week(), MADRID, 7);
        assert_eq!(found.len(), 1, "an instant is still on the calendar");
    }

    #[test]
    fn the_day_containing_a_dst_transition_is_measured_against_its_own_midnight() {
        // 29 March 2026, Madrid: the day is 23 hours long. A 09:00 meeting is still 540
        // minutes from that day's midnight, because the hour vanished at 02:00.
        let week_start = NaiveDate::from_ymd_opt(2026, 3, 23).unwrap();
        let found = segments(
            at("2026-03-29 09:00:00"),
            at("2026-03-29 10:00:00"),
            week_start,
            MADRID,
            7,
        );
        assert_eq!(found[0].day, 6, "Sunday");
        assert_eq!(
            found[0].top_minutes, 480.0,
            "eight hours of real time have passed since a midnight that lost an hour"
        );
    }

    #[test]
    fn events_that_do_not_overlap_each_take_the_full_width() {
        let assignment = columns(&[segment(540.0, 60.0), segment(660.0, 60.0)]);
        assert_eq!(assignment, vec![(0, 1), (0, 1)]);
    }

    #[test]
    fn two_overlapping_events_share_the_width_rather_than_covering_each_other() {
        let assignment = columns(&[segment(540.0, 60.0), segment(570.0, 60.0)]);
        assert_eq!(assignment, vec![(0, 2), (1, 2)]);
    }

    #[test]
    fn three_mutually_overlapping_events_split_three_ways() {
        let assignment = columns(&[
            segment(540.0, 120.0),
            segment(560.0, 60.0),
            segment(580.0, 60.0),
        ]);
        assert_eq!(assignment.iter().map(|(_, width)| *width).max(), Some(3));
        let used: std::collections::HashSet<usize> =
            assignment.iter().map(|(column, _)| *column).collect();
        assert_eq!(used.len(), 3, "each gets its own column");
    }

    #[test]
    fn a_column_is_reused_once_its_occupant_has_ended() {
        // Otherwise a busy morning keeps narrowing the whole day.
        let assignment = columns(&[
            segment(540.0, 60.0),
            segment(540.0, 60.0),
            segment(600.0, 60.0),
        ]);
        assert_eq!(
            assignment[2].0, 0,
            "the third slots back into the first column"
        );
    }

    #[test]
    fn a_quiet_afternoon_is_not_narrowed_by_a_busy_morning() {
        let assignment = columns(&[
            segment(540.0, 60.0),
            segment(550.0, 60.0),
            segment(900.0, 60.0),
        ]);
        assert_eq!(assignment[2], (0, 1), "a separate cluster is full width");
    }
}
