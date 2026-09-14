//! How many days the grid shows, and where those days start.
//!
//! The set is discrete and the rules differ per span rather than generalising: spans 1 and 3
//! anchor on the focused day, spans 5 and 7 anchor on its Monday. The step follows the
//! anchor, not the width — stepping five days from a Monday lands on a Saturday, and a
//! working week that starts on Saturday is not a working week.

use chrono::{Datelike, Duration, NaiveDate};

/// The Monday on or before `date`. Weeks start on Monday here; Sunday-first is a preference
/// nobody has asked for, and guessing it from the locale would be a guess.
pub fn monday_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday() as i64)
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Span {
    Day,
    Three,
    Work,
    #[default]
    Week,
}

impl Span {
    pub const ALL: [Span; 4] = [Span::Day, Span::Three, Span::Work, Span::Week];

    /// Columns drawn.
    pub fn days(self) -> usize {
        match self {
            Span::Day => 1,
            Span::Three => 3,
            Span::Work => 5,
            Span::Week => 7,
        }
    }

    /// The first column's date, given the day the user is looking at.
    pub fn start_for(self, focus: NaiveDate) -> NaiveDate {
        match self {
            Span::Day | Span::Three => focus,
            Span::Work | Span::Week => monday_of(focus),
        }
    }

    /// Days moved by one press of previous/next. Not `days()`: a working week steps a whole
    /// week, or the next press would start it on a Saturday.
    pub fn step(self) -> i64 {
        match self {
            Span::Day => 1,
            Span::Three => 3,
            Span::Work | Span::Week => 7,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Span::Day => "Day",
            Span::Three => "3 days",
            Span::Work => "Work week",
            Span::Week => "Week",
        }
    }

    /// Stable across releases: this is what lands in `settings.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Span::Day => "day",
            Span::Three => "three",
            Span::Work => "work",
            Span::Week => "week",
        }
    }

    pub fn from_key(key: &str) -> Option<Span> {
        Span::ALL.into_iter().find(|span| span.key() == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn each_span_anchors_where_spec_views_timegrid_says_it_does() {
        // Thursday 2026-09-17; its Monday is 2026-09-14.
        let thursday = date("2026-09-17");
        assert_eq!(Span::Day.start_for(thursday), thursday);
        assert_eq!(Span::Three.start_for(thursday), thursday);
        assert_eq!(Span::Work.start_for(thursday), date("2026-09-14"));
        assert_eq!(Span::Week.start_for(thursday), date("2026-09-14"));
    }

    #[test]
    fn a_working_week_steps_seven_days_not_five() {
        // Stepping five from a Monday lands on a Saturday, which is the bug this exists to
        // prevent: the span is Monday-to-Friday, not "five days from wherever".
        let monday = date("2026-09-14");
        let next = monday + Duration::days(Span::Work.step());
        assert_eq!(next, date("2026-09-21"));
        assert_eq!(Span::Work.start_for(next), date("2026-09-21"));
    }

    #[test]
    fn stepping_a_three_day_span_moves_three_days() {
        let start = date("2026-09-17");
        assert_eq!(
            start + Duration::days(Span::Three.step()),
            date("2026-09-20")
        );
    }

    #[test]
    fn a_monday_anchors_to_itself() {
        let monday = date("2026-09-14");
        for span in Span::ALL {
            assert_eq!(span.start_for(monday), monday, "{span:?}");
        }
    }

    #[test]
    fn keys_round_trip_and_are_all_distinct() {
        for span in Span::ALL {
            assert_eq!(Span::from_key(span.key()), Some(span));
        }
        assert_eq!(Span::from_key("fortnight"), None);
    }

    #[test]
    fn the_column_counts_are_the_discrete_set_the_user_asked_for() {
        let counts: Vec<usize> = Span::ALL.into_iter().map(Span::days).collect();
        assert_eq!(counts, vec![1, 3, 5, 7]);
    }
}
