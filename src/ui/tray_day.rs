//! The compact day view opened from the tray.
//!
//! It is the time grid with one column and a bounded vertical window, not a second grid.
//! What is new here is only which slice of the day opens first.

/// Minutes in a day.
const DAY: f64 = 24.0 * 60.0;

pub const DEFAULT_BEFORE_HOURS: f64 = 1.0;
pub const DEFAULT_AFTER_HOURS: f64 = 4.0;

/// The slice of the day to open on, in minutes past midnight.
///
/// One hour back and four forward by default. Near either end of the day the slice would run
/// off it, so it slides along the day rather than shrinking: a five-hour window that becomes
/// a one-hour window at 23:30 shows the user almost nothing, which is the opposite of the
/// point. It only shrinks if the whole day is shorter than the requested span.
pub fn window(now_minutes: f64, before_hours: f64, after_hours: f64) -> (f64, f64) {
    let span = ((before_hours + after_hours) * 60.0).clamp(0.0, DAY);
    let now = now_minutes.clamp(0.0, DAY);

    let mut start = now - before_hours * 60.0;
    if start < 0.0 {
        start = 0.0;
    }
    if start + span > DAY {
        start = DAY - span;
    }

    (start.max(0.0), (start + span).min(DAY))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: f64) -> f64 {
        hour * 60.0
    }

    #[test]
    fn the_default_window_is_an_hour_back_and_four_forward() {
        let (start, end) = window(at(14.0), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
        assert_eq!((start, end), (at(13.0), at(18.0)));
    }

    #[test]
    fn early_morning_slides_the_window_rather_than_shrinking_it() {
        // 00:30 — an hour back is yesterday. The window starts at midnight and keeps its
        // five hours, because a window that shrinks to half an hour shows nothing.
        let (start, end) = window(at(0.5), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
        assert_eq!((start, end), (at(0.0), at(5.0)));
        assert_eq!(end - start, at(5.0));
    }

    #[test]
    fn late_evening_slides_the_window_back_off_the_end() {
        // 23:30 — four hours forward is tomorrow. The window ends at midnight and keeps its
        // five hours by starting at 19:00.
        let (start, end) = window(at(23.5), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
        assert_eq!((start, end), (at(19.0), at(24.0)));
        assert_eq!(end - start, at(5.0));
    }

    #[test]
    fn the_window_keeps_its_span_at_every_minute_of_the_day() {
        for minute in 0..(24 * 60) {
            let (start, end) = window(f64::from(minute), DEFAULT_BEFORE_HOURS, DEFAULT_AFTER_HOURS);
            assert_eq!(end - start, at(5.0), "shrank at minute {minute}");
            assert!(start >= 0.0 && end <= DAY, "ran off the day at {minute}");
        }
    }

    #[test]
    fn a_span_longer_than_a_day_is_the_whole_day() {
        let (start, end) = window(at(12.0), 20.0, 20.0);
        assert_eq!((start, end), (0.0, DAY));
    }

    #[test]
    fn a_configured_span_is_honoured() {
        let (start, end) = window(at(12.0), 2.0, 2.0);
        assert_eq!((start, end), (at(10.0), at(14.0)));
    }
}
