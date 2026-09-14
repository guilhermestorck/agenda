//! The month grid: whole weeks, one cell per day, events as chips.
//!
//! No hour axis, so none of `ui::vertical` applies here — the compressed quiet hours and the
//! lead-in band are the time grid's business. What this module needs is which cells an event
//! occupies, and `layout::segments` already answers that: it splits an occurrence into
//! per-day pieces clamped to a window, and does not care whether the window is seven days or
//! forty-two.

use chrono::{Datelike, Duration, NaiveDate};
use chrono_tz::Tz;

use super::layout::segments;
use super::span::monday_of;
use super::week::Item;

/// Six weeks, always. Five would fit some months and not others, and a grid that changes
/// height between September and October reflows the whole window on every navigation.
pub const CELLS: usize = 42;

/// The date the grid starts on: the Monday on or before the first of the month.
pub fn grid_start(any_day_in_month: NaiveDate) -> NaiveDate {
    let first = any_day_in_month.with_day(1).unwrap_or(any_day_in_month);
    monday_of(first)
}

/// The events in each of the forty-two cells, in start order.
///
/// A multi-day event appears in every cell it covers, which is what `segments` already
/// produces — one piece per day touched.
pub fn cells(items: &[Item], start: NaiveDate, zone: Tz) -> Vec<Vec<Item>> {
    let mut cells: Vec<Vec<Item>> = vec![Vec::new(); CELLS];
    for item in items {
        for segment in segments(item.start_utc, item.end_utc, start, zone, CELLS) {
            if let Some(cell) = cells.get_mut(segment.day) {
                cell.push(item.clone());
            }
        }
    }
    for cell in &mut cells {
        cell.sort_by(|a, b| {
            b.all_day
                .cmp(&a.all_day)
                .then(a.start_utc.cmp(&b.start_utc))
                .then_with(|| a.summary.cmp(&b.summary))
        });
    }
    cells
}

/// How many chips fit, and how many are left over.
///
/// The overflow is counted, never estimated: "+3 more" that is really four is worse than no
/// indicator at all, because the user stops checking.
pub fn visible_and_overflow(total: usize, room: usize) -> (usize, usize) {
    if total <= room {
        return (total, 0);
    }
    // One slot goes to the "+N more" chip itself, or the count would omit the event it
    // displaced.
    let shown = room.saturating_sub(1);
    (shown, total - shown)
}

/// Whether a cell belongs to the month being shown, rather than the days either side.
pub fn in_month(cell: usize, start: NaiveDate, month: u32) -> bool {
    (start + Duration::days(cell as i64)).month() == month
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::style::Colors;

    const MADRID: Tz = chrono_tz::Europe::Madrid;

    fn on(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn at(local: &str) -> i64 {
        let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").unwrap();
        use chrono::TimeZone;
        MADRID
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn item(summary: &str, from: &str, to: &str, all_day: bool) -> Item {
        Item {
            summary: summary.to_string(),
            start_utc: at(from),
            end_utc: at(to),
            all_day,
            colors: Colors {
                fill: "#3584e4".to_string(),
                marker: "#3584e4".to_string(),
            },
            account: "work@example.com".to_string(),
            picture: None,
        }
    }

    #[test]
    fn the_grid_starts_on_the_monday_on_or_before_the_first() {
        // September 2026 opens on a Tuesday, so the grid starts on 31 August.
        assert_eq!(grid_start(on("2026-09-20")), on("2026-08-31"));
    }

    #[test]
    fn a_month_that_opens_on_a_monday_starts_on_that_monday() {
        // June 2026 opens on a Monday: no leading days from May.
        assert_eq!(grid_start(on("2026-06-15")), on("2026-06-01"));
    }

    #[test]
    fn every_month_gets_the_same_forty_two_cells() {
        // February 2027 is 28 days opening on a Monday — the shortest possible grid, and it
        // still gets six rows so the layout never changes height.
        for day in ["2026-09-01", "2027-02-10", "2026-06-01", "2028-02-29"] {
            let cells = cells(&[], grid_start(on(day)), MADRID);
            assert_eq!(cells.len(), CELLS, "{day}");
        }
    }

    #[test]
    fn a_multi_day_event_appears_in_every_cell_it_covers() {
        // Thursday to Saturday, three cells.
        let items = vec![item(
            "conference",
            "2026-09-17 09:00:00",
            "2026-09-19 18:00:00",
            false,
        )];
        let start = grid_start(on("2026-09-01"));
        let cells = cells(&items, start, MADRID);
        let occupied: Vec<usize> = cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.is_empty())
            .map(|(index, _)| index)
            .collect();
        // 31 Aug is cell 0, so 17 September is cell 17.
        assert_eq!(occupied, vec![17, 18, 19]);
    }

    #[test]
    fn days_outside_the_month_are_still_filled_and_still_marked_as_outside() {
        let start = grid_start(on("2026-09-01"));
        assert!(!in_month(0, start, 9), "31 August is not September");
        assert!(in_month(1, start, 9), "1 September is");
        let items = vec![item(
            "august",
            "2026-08-31 10:00:00",
            "2026-08-31 11:00:00",
            false,
        )];
        assert!(!cells(&items, start, MADRID)[0].is_empty(), "still drawn");
    }

    #[test]
    fn overflow_is_counted_not_estimated() {
        assert_eq!(visible_and_overflow(3, 5), (3, 0));
        assert_eq!(visible_and_overflow(5, 5), (5, 0));
        // Six into five: one slot goes to the indicator, so four show and two are hidden.
        assert_eq!(visible_and_overflow(6, 5), (4, 2));
        assert_eq!(visible_and_overflow(8, 3), (2, 6));
    }

    #[test]
    fn the_overflow_count_always_adds_up() {
        for total in 0..20usize {
            for room in 1..8usize {
                let (shown, hidden) = visible_and_overflow(total, room);
                assert_eq!(shown + hidden, total, "total={total} room={room}");
                assert!(shown <= room, "overflowed the cell");
            }
        }
    }

    #[test]
    fn all_day_events_lead_a_cell() {
        let items = vec![
            item(
                "standup",
                "2026-09-16 09:00:00",
                "2026-09-16 09:15:00",
                false,
            ),
            item(
                "holiday",
                "2026-09-16 02:00:00",
                "2026-09-17 02:00:00",
                true,
            ),
        ];
        let cells = cells(&items, grid_start(on("2026-09-01")), MADRID);
        let names: Vec<&str> = cells[16].iter().map(|i| i.summary.as_str()).collect();
        assert_eq!(names.first(), Some(&"holiday"));
    }
}
