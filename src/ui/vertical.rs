//! Where a minute of the day sits, in pixels.
//!
//! Not `minutes / 60 * HOUR_HEIGHT`. Hours outside a core band are drawn at half height so a
//! day is graspable without scrolling, and an hour holding an event is drawn full height even
//! when it falls outside that band. The mapping is therefore piecewise and depends on the
//! day's contents.
//!
//! One mapping serves the whole grid. The hour axis is a single column shared by every
//! column of days, so it cannot claim 06:00 is tall for Monday and short for Tuesday: an
//! hour expands if *any* visible day has an event in it. In a wide span that means
//! compression is often inert, which is accepted — see SPEC-views-timegrid.
//!
//! Everything here is a pure function of (minutes, occupied hours, core band) so it is
//! testable without a widget, which matters because getting it wrong moves every event.

use std::collections::BTreeSet;

/// Pixels per hour at full height.
pub const HOUR_HEIGHT: f64 = 48.0;
/// What a quiet hour shrinks to. Half keeps a 30-minute event 12px tall — still readable,
/// still clickable.
pub const QUIET_SCALE: f64 = 0.5;

pub const DEFAULT_CORE_START: u32 = 8;
pub const DEFAULT_CORE_END: u32 = 22;

/// The hours drawn at full height regardless of content.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Core {
    pub start: u32,
    pub end: u32,
}

impl Default for Core {
    fn default() -> Self {
        Self {
            start: DEFAULT_CORE_START,
            end: DEFAULT_CORE_END,
        }
    }
}

impl Core {
    fn contains(self, hour: u32) -> bool {
        (self.start..self.end).contains(&hour)
    }
}

/// The height of one hour row.
pub fn hour_height(hour: u32, occupied: &BTreeSet<u32>, core: Core) -> f64 {
    if core.contains(hour) || occupied.contains(&hour) {
        HOUR_HEIGHT
    } else {
        HOUR_HEIGHT * QUIET_SCALE
    }
}

/// The y offset of `minutes` past midnight.
///
/// Heights are always the difference of two of these, never a duration times a scale: an
/// event from 07:00 to 09:00 crosses the boundary, and `duration × scale` is wrong for it
/// in both directions depending on which side you scale from.
pub fn y_for(minutes: f64, occupied: &BTreeSet<u32>, core: Core) -> f64 {
    let minutes = minutes.clamp(0.0, 24.0 * 60.0);
    let whole = (minutes / 60.0).floor() as u32;
    let mut y = (0..whole.min(24))
        .map(|hour| hour_height(hour, occupied, core))
        .sum();
    if whole < 24 {
        let remainder = minutes - f64::from(whole) * 60.0;
        y += remainder / 60.0 * hour_height(whole, occupied, core);
    }
    y
}

/// The full drawn height of a day.
pub fn day_height(occupied: &BTreeSet<u32>, core: Core) -> f64 {
    y_for(24.0 * 60.0, occupied, core)
}

/// Which hours hold an event, across every day on screen.
///
/// Takes minute ranges rather than events so it stays free of the grid's types — the caller
/// has already split occurrences into per-day segments by then.
pub fn occupied_hours(spans: impl IntoIterator<Item = (f64, f64)>) -> BTreeSet<u32> {
    let mut hours = BTreeSet::new();
    for (top, height) in spans {
        if height <= 0.0 {
            continue;
        }
        let first = (top / 60.0).floor().clamp(0.0, 23.0) as u32;
        // An event ending exactly on the hour does not occupy the hour it ends on.
        let last = (((top + height) - 0.001) / 60.0).floor().clamp(0.0, 23.0) as u32;
        for hour in first..=last {
            hours.insert(hour);
        }
    }
    hours
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> BTreeSet<u32> {
        BTreeSet::new()
    }

    fn at(hours: &[u32]) -> BTreeSet<u32> {
        hours.iter().copied().collect()
    }

    #[test]
    fn core_hours_are_full_height_whether_occupied_or_not() {
        for hour in 8..22 {
            assert_eq!(
                hour_height(hour, &none(), Core::default()),
                HOUR_HEIGHT,
                "{hour}"
            );
        }
    }

    #[test]
    fn a_quiet_empty_hour_is_half_height() {
        for hour in [0, 6, 7, 22, 23] {
            assert_eq!(
                hour_height(hour, &none(), Core::default()),
                HOUR_HEIGHT * QUIET_SCALE,
                "{hour}"
            );
        }
    }

    #[test]
    fn a_quiet_hour_holding_an_event_is_full_height() {
        assert_eq!(hour_height(6, &at(&[6]), Core::default()), HOUR_HEIGHT);
        assert_eq!(hour_height(23, &at(&[23]), Core::default()), HOUR_HEIGHT);
        // Its neighbours are untouched.
        assert_eq!(
            hour_height(5, &at(&[6]), Core::default()),
            HOUR_HEIGHT * QUIET_SCALE
        );
    }

    #[test]
    fn the_mapping_never_goes_backwards() {
        let occupied = at(&[6, 23]);
        let mut previous = -1.0;
        for minute in (0..=24 * 60).step_by(7) {
            let y = y_for(f64::from(minute), &occupied, Core::default());
            assert!(
                y >= previous,
                "went backwards at {minute}: {y} < {previous}"
            );
            previous = y;
        }
    }

    #[test]
    fn an_event_crossing_the_core_boundary_is_measured_through_the_mapping() {
        // 07:00 to 09:00. The first hour is quiet and half height, the second is core and
        // full — so the event is 1.5 hours tall, not 2, and not 1.
        let core = Core::default();
        let height = y_for(9.0 * 60.0, &none(), core) - y_for(7.0 * 60.0, &none(), core);
        assert_eq!(height, HOUR_HEIGHT * 1.5);

        // Naively scaling the duration gets it wrong from either side.
        assert_ne!(height, 2.0 * HOUR_HEIGHT);
        assert_ne!(height, 2.0 * HOUR_HEIGHT * QUIET_SCALE);
    }

    #[test]
    fn an_event_crossing_the_evening_boundary_is_measured_the_same_way() {
        // 21:00 to 23:00: core hour then quiet hour.
        let core = Core::default();
        let height = y_for(23.0 * 60.0, &none(), core) - y_for(21.0 * 60.0, &none(), core);
        assert_eq!(height, HOUR_HEIGHT * 1.5);
    }

    #[test]
    fn a_day_with_nothing_quiet_saves_five_hours_of_scrolling() {
        // Ten quiet hours at half height: 00-07 and 22-23 is 10 hours, saving 5.
        let compressed = day_height(&none(), Core::default());
        let uncompressed = 24.0 * HOUR_HEIGHT;
        assert_eq!(uncompressed - compressed, 5.0 * HOUR_HEIGHT);
    }

    #[test]
    fn expanding_every_quiet_hour_gives_back_the_uncompressed_day() {
        let all: BTreeSet<u32> = (0..24).collect();
        assert_eq!(day_height(&all, Core::default()), 24.0 * HOUR_HEIGHT);
    }

    #[test]
    fn occupancy_covers_every_hour_an_event_touches() {
        // 07:00 for 150 minutes reaches 09:30, so hours 7, 8 and 9.
        assert_eq!(occupied_hours([(7.0 * 60.0, 150.0)]), at(&[7, 8, 9]));
    }

    #[test]
    fn an_event_ending_on_the_hour_does_not_occupy_that_hour() {
        // 06:00 to 07:00 occupies 6, not 7 — otherwise a single early meeting expands two
        // hours and the compression is quietly lost.
        assert_eq!(occupied_hours([(6.0 * 60.0, 60.0)]), at(&[6]));
    }

    #[test]
    fn a_zero_length_event_occupies_nothing() {
        assert_eq!(occupied_hours([(6.0 * 60.0, 0.0)]), none());
    }

    #[test]
    fn a_custom_core_band_moves_the_compression_with_it() {
        let core = Core { start: 6, end: 20 };
        assert_eq!(hour_height(6, &none(), core), HOUR_HEIGHT);
        assert_eq!(hour_height(21, &none(), core), HOUR_HEIGHT * QUIET_SCALE);
    }
}
