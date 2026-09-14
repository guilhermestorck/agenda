//! When to remind the user, and about what.
//!
//! The lead time cascades, most specific winning:
//!
//! ```text
//! the event's own reminder   (Google's, read from the API)
//!   ↳ the calendar's setting (the user's)
//!       ↳ the account's setting (the user's)
//!           ↳ the global default
//! ```
//!
//! The same shape as the colour rule in `accounts::style`, extended by a level at each end.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Duration, TimeZone, Utc};
use chrono_tz::Tz;

use crate::config::parse_pairs;
use crate::recur::Occurrence;

/// Ten minutes: long enough to finish a sentence and walk to a meeting, short enough that a
/// reminder still feels like one.
pub const DEFAULT_LEAD_MINUTES: i64 = 10;
/// All-day events have no start time to count back from, so their reminder is anchored to an
/// hour of the morning instead. A notification at midnight is one you sleep through.
pub const DEFAULT_ALL_DAY_HOUR: u32 = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub lead_minutes: i64,
    pub all_day_hour: u32,
    /// The hours the grid draws at full height. Outside them a row is halved, unless it
    /// holds an event. See `ui::vertical`.
    pub core_hours_start: u32,
    pub core_hours_end: u32,
    /// The zone the grid is drawn in. `None` means the system's.
    pub timezone: Option<String>,
    /// An optional second zone, shown beside the first on the hour axis and on agenda rows.
    pub secondary_timezone: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lead_minutes: DEFAULT_LEAD_MINUTES,
            all_day_hour: DEFAULT_ALL_DAY_HOUR,
            core_hours_start: crate::ui::vertical::DEFAULT_CORE_START,
            core_hours_end: crate::ui::vertical::DEFAULT_CORE_END,
            timezone: None,
            secondary_timezone: None,
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Self> {
        let pairs = parse_pairs(text)?;
        let mut settings = Self::default();
        if let Some(value) = pairs.get("notify_lead_minutes") {
            settings.lead_minutes = value
                .parse()
                .with_context(|| format!("notify_lead_minutes is not a number: {value}"))?;
        }
        if let Some(value) = pairs.get("all_day_notify_hour") {
            let hour: u32 = value
                .parse()
                .with_context(|| format!("all_day_notify_hour is not a number: {value}"))?;
            anyhow::ensure!(hour < 24, "all_day_notify_hour must be an hour of the day");
            settings.all_day_hour = hour;
        }
        for (key, field) in [("core_hours_start", 0usize), ("core_hours_end", 1usize)] {
            if let Some(value) = pairs.get(key) {
                let hour: u32 = value
                    .parse()
                    .with_context(|| format!("{key} is not a number: {value}"))?;
                anyhow::ensure!(hour <= 24, "{key} must be an hour of the day");
                if field == 0 {
                    settings.core_hours_start = hour;
                } else {
                    settings.core_hours_end = hour;
                }
            }
        }
        settings.timezone = pairs.get("timezone").cloned();
        settings.secondary_timezone = pairs.get("secondary_timezone").cloned();
        anyhow::ensure!(
            settings.core_hours_start < settings.core_hours_end,
            "core_hours_start must come before core_hours_end"
        );
        Ok(settings)
    }

    /// Absent or unreadable settings fall back to the defaults with a warning. A typo in a
    /// preferences file should cost the user their preference, not their reminders.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::parse(&text) {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), path = %path.display(), "using default settings");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }
}

/// Resolve the lead time for one occurrence.
pub fn lead_minutes(
    event_reminder: Option<i64>,
    calendar: Option<i64>,
    account: Option<i64>,
    global: i64,
) -> i64 {
    event_reminder
        .or(calendar)
        .or(account)
        .unwrap_or(global)
        // A negative lead would schedule a reminder after the event, which is not a reminder.
        .max(0)
}

/// When to fire, as a UTC instant.
///
/// A timed event counts back from its start. An all-day event has no start time to count
/// back from, so the lead is read as whole days and anchored to `all_day_hour` in the user's
/// own zone: a lead under a day fires that morning, a day or more fires that many mornings
/// earlier.
pub fn notify_at(
    occurrence: &Occurrence,
    lead_minutes: i64,
    all_day_hour: u32,
    zone: Tz,
) -> Option<i64> {
    if !occurrence.event.all_day {
        return Some(occurrence.start_utc - lead_minutes * 60);
    }

    let start = Utc.timestamp_opt(occurrence.start_utc, 0).single()?;
    let date = start.with_timezone(&zone).date_naive() - Duration::days(lead_minutes / 1440);
    let local = date.and_hms_opt(all_day_hour, 0, 0)?;
    resolve_local(local, zone)
}

/// Turn a local wall-clock time into an instant, whatever the calendar did to that morning.
///
/// A fall-back makes an hour ambiguous, and the earlier instant is the right one. A
/// spring-forward deletes an hour outright, and `chrono` then offers *neither* an earliest
/// nor a latest — so the chosen hour is nudged forward until it exists again. Returning
/// `None` there would silently drop a reminder once a year.
fn resolve_local(local: chrono::NaiveDateTime, zone: Tz) -> Option<i64> {
    for hours in 0..4 {
        let candidate = local + Duration::hours(hours);
        if let Some(instant) = zone.from_local_datetime(&candidate).earliest() {
            return Some(instant.timestamp());
        }
    }
    None
}

/// Everything due to be notified in `(after, now]`.
///
/// A window rather than an instant, because the scheduler cannot assume it was running:
/// a suspend across a reminder's time must still deliver it on resume, as long as the event
/// itself has not already passed.
pub fn due(occurrences: &[(Occurrence, i64)], after: i64, now: i64) -> Vec<&Occurrence> {
    occurrences
        .iter()
        .filter(|(occurrence, at)| *at > after && *at <= now && occurrence.end_utc > now)
        .map(|(occurrence, _)| occurrence)
        .collect()
}

/// Show one reminder on the desktop.
///
/// Best effort: a notification daemon that is not running, or refuses, must not take the
/// calendar down with it.
pub fn show(summary: &str, body: &str) {
    let result = notify_rust::Notification::new()
        .summary(summary)
        .body(body)
        .icon("x-office-calendar-symbolic")
        .appname("agenda")
        // Reminders are time-bound; one that outlives its meeting is clutter.
        .timeout(notify_rust::Timeout::Milliseconds(15_000))
        .show();
    if let Err(error) = result {
        tracing::warn!(%error, "could not show a notification");
    }
}

/// The line under the title: when it starts, and where, when Google says.
pub fn body_for(occurrence: &Occurrence, zone: Tz, now: i64) -> String {
    let when = if occurrence.event.all_day {
        "Today".to_string()
    } else {
        let minutes = (occurrence.start_utc - now).max(0) / 60;
        let clock = Utc
            .timestamp_opt(occurrence.start_utc, 0)
            .single()
            .map(|instant| instant.with_timezone(&zone).format("%H:%M").to_string())
            .unwrap_or_default();
        match minutes {
            0 => format!("Starting now, at {clock}"),
            1 => format!("In 1 minute, at {clock}"),
            _ => format!("In {minutes} minutes, at {clock}"),
        }
    };
    match occurrence.event.location.as_deref() {
        Some(location) if !location.is_empty() => format!("{when}\n{location}"),
        _ => when,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Event;

    const MADRID: Tz = chrono_tz::Europe::Madrid;

    fn at(local: &str) -> i64 {
        let naive = chrono::NaiveDateTime::parse_from_str(local, "%Y-%m-%d %H:%M:%S").unwrap();
        MADRID
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn occurrence(start: i64, end: i64, all_day: bool) -> Occurrence {
        Occurrence {
            start_utc: start,
            end_utc: end,
            event: Event {
                account: "work@example.com".to_string(),
                calendar_id: "primary".to_string(),
                id: "e".to_string(),
                ical_uid: None,
                etag: None,
                summary: "Standup".to_string(),
                description: None,
                location: None,
                start_utc: start,
                end_utc: end,
                timezone: Some("Europe/Madrid".to_string()),
                all_day,
                rrule: None,
                recurring_event_id: None,
                original_start_utc: None,
                status: "confirmed".to_string(),
                reminder_minutes: None,
                updated_at: None,
            },
        }
    }

    #[test]
    fn the_core_band_is_read_from_the_settings_file() {
        let settings =
            Settings::parse("core_hours_start = \"6\"\ncore_hours_end = \"20\"").unwrap();
        assert_eq!(settings.core_hours_start, 6);
        assert_eq!(settings.core_hours_end, 20);
    }

    #[test]
    fn a_core_band_that_ends_before_it_starts_is_rejected() {
        // Silently swapping them would draw a grid nobody asked for; a typo should cost the
        // preference, not the layout.
        assert!(Settings::parse("core_hours_start = \"20\"\ncore_hours_end = \"8\"").is_err());
    }

    #[test]
    fn the_events_own_reminder_beats_every_other_level() {
        assert_eq!(lead_minutes(Some(5), Some(15), Some(30), 10), 5);
    }

    #[test]
    fn a_calendar_setting_beats_its_accounts() {
        assert_eq!(lead_minutes(None, Some(15), Some(30), 10), 15);
    }

    #[test]
    fn an_account_setting_beats_the_global_default() {
        assert_eq!(lead_minutes(None, None, Some(30), 10), 30);
    }

    #[test]
    fn the_global_default_applies_when_nothing_else_is_set() {
        assert_eq!(lead_minutes(None, None, None, 10), 10);
    }

    #[test]
    fn a_zero_lead_is_a_choice_and_not_an_absent_one() {
        // "Tell me as it starts" must not fall through to the level above.
        assert_eq!(lead_minutes(Some(0), Some(15), Some(30), 10), 0);
        assert_eq!(lead_minutes(None, Some(0), Some(30), 10), 0);
    }

    #[test]
    fn a_negative_lead_never_schedules_a_reminder_after_the_event() {
        assert_eq!(lead_minutes(Some(-30), None, None, 10), 0);
    }

    #[test]
    fn a_timed_event_counts_back_from_its_start() {
        let event = occurrence(at("2026-09-14 09:30:00"), at("2026-09-14 09:45:00"), false);
        assert_eq!(
            notify_at(&event, 10, 9, MADRID),
            Some(at("2026-09-14 09:20:00"))
        );
    }

    #[test]
    fn an_all_day_event_is_announced_on_the_morning_of() {
        let event = occurrence(at("2026-09-15 00:00:00"), at("2026-09-16 00:00:00"), true);
        assert_eq!(
            notify_at(&event, 10, 9, MADRID),
            Some(at("2026-09-15 09:00:00")),
            "a lead under a day means that morning, not the midnight before"
        );
    }

    #[test]
    fn an_all_day_event_with_a_days_lead_is_announced_that_many_mornings_earlier() {
        let event = occurrence(at("2026-09-15 00:00:00"), at("2026-09-16 00:00:00"), true);
        assert_eq!(
            notify_at(&event, 1440, 9, MADRID),
            Some(at("2026-09-14 09:00:00"))
        );
        assert_eq!(
            notify_at(&event, 2880, 9, MADRID),
            Some(at("2026-09-13 09:00:00"))
        );
    }

    #[test]
    fn the_all_day_hour_is_the_users_hour_in_the_users_zone() {
        let event = occurrence(at("2026-09-15 00:00:00"), at("2026-09-16 00:00:00"), true);
        assert_eq!(
            notify_at(&event, 0, 7, MADRID),
            Some(at("2026-09-15 07:00:00"))
        );
    }

    #[test]
    fn an_all_day_reminder_survives_a_morning_that_loses_an_hour() {
        // 29 March 2026, Madrid: 02:00 becomes 03:00, so 02:00 that day does not exist.
        // chrono offers neither an earliest nor a latest for a gap, so a naive lookup
        // returns None and the reminder vanishes — once a year, silently.
        let event = occurrence(at("2026-03-30 00:00:00"), at("2026-03-31 00:00:00"), true);
        assert_eq!(
            notify_at(&event, 1440, 2, MADRID),
            Some(at("2026-03-29 03:00:00")),
            "nudged to the first hour that exists, still that morning"
        );
    }

    #[test]
    fn an_all_day_reminder_on_an_ambiguous_morning_takes_the_earlier_instant() {
        // 25 October 2026, Madrid: 03:00 becomes 02:00, so 02:30 happens twice.
        let event = occurrence(at("2026-10-26 00:00:00"), at("2026-10-27 00:00:00"), true);
        let fired = notify_at(&event, 1440, 2, MADRID).unwrap();
        let local = Utc.timestamp_opt(fired, 0).unwrap().with_timezone(&MADRID);
        assert_eq!(chrono::Timelike::hour(&local), 2);
        assert_eq!(
            chrono::Datelike::day(&local),
            25,
            "the reminder is on the right morning either way"
        );
    }

    #[test]
    fn a_reminder_whose_time_passed_during_a_suspend_is_still_delivered() {
        // SPEC §2.9: across a suspend/resume cycle that spans the scheduled time.
        let event = occurrence(at("2026-09-14 09:30:00"), at("2026-09-14 09:45:00"), false);
        let scheduled = notify_at(&event, 10, 9, MADRID).unwrap();
        let pairs = [(event, scheduled)];
        let due_now = due(&pairs, at("2026-09-14 08:00:00"), at("2026-09-14 09:25:00"));
        assert_eq!(due_now.len(), 1, "the machine was asleep at 09:20");
    }

    #[test]
    fn a_reminder_for_an_event_that_has_already_ended_is_dropped() {
        // Waking to be told about a meeting that finished while you slept helps nobody.
        let event = occurrence(at("2026-09-14 09:30:00"), at("2026-09-14 09:45:00"), false);
        let scheduled = notify_at(&event, 10, 9, MADRID).unwrap();
        let pairs = [(event, scheduled)];
        let due_now = due(&pairs, at("2026-09-14 08:00:00"), at("2026-09-14 11:00:00"));
        assert!(due_now.is_empty());
    }

    #[test]
    fn a_reminder_is_delivered_once_and_not_again() {
        let event = occurrence(at("2026-09-14 09:30:00"), at("2026-09-14 09:45:00"), false);
        let scheduled = notify_at(&event, 10, 9, MADRID).unwrap();
        let pairs = [(event, scheduled)];

        let first = due(&pairs, at("2026-09-14 09:15:00"), at("2026-09-14 09:21:00"));
        assert_eq!(first.len(), 1);
        let again = due(&pairs, at("2026-09-14 09:21:00"), at("2026-09-14 09:25:00"));
        assert!(again.is_empty(), "the window has moved past it");
    }

    #[test]
    fn a_reminder_still_ahead_is_not_delivered_early() {
        let event = occurrence(at("2026-09-14 09:30:00"), at("2026-09-14 09:45:00"), false);
        let scheduled = notify_at(&event, 10, 9, MADRID).unwrap();
        let pairs = [(event, scheduled)];
        let due_now = due(&pairs, at("2026-09-14 09:00:00"), at("2026-09-14 09:05:00"));
        assert!(due_now.is_empty());
    }

    #[test]
    fn the_body_says_when_and_where() {
        let mut event = occurrence(at("2026-09-14 09:30:00"), at("2026-09-14 09:45:00"), false);
        event.event.location = Some("Meeting room 2".to_string());
        let body = body_for(&event, MADRID, at("2026-09-14 09:20:00"));
        assert!(body.contains("In 10 minutes"), "got {body}");
        assert!(body.contains("09:30"), "got {body}");
        assert!(body.contains("Meeting room 2"), "got {body}");
    }

    #[test]
    fn a_body_with_no_location_does_not_leave_a_blank_line() {
        let event = occurrence(at("2026-09-14 09:30:00"), at("2026-09-14 09:45:00"), false);
        let body = body_for(&event, MADRID, at("2026-09-14 09:20:00"));
        assert!(!body.contains('\n'), "got {body:?}");
    }

    #[test]
    fn an_all_day_reminder_does_not_pretend_to_have_a_start_time() {
        let event = occurrence(at("2026-09-15 00:00:00"), at("2026-09-16 00:00:00"), true);
        let body = body_for(&event, MADRID, at("2026-09-15 09:00:00"));
        assert_eq!(body, "Today");
    }

    #[test]
    fn absent_settings_are_the_defaults_not_an_error() {
        let settings = Settings::load(Path::new("/nonexistent/settings.toml"));
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn settings_are_read_from_the_documented_keys() {
        let settings = Settings::parse(
            "# my preferences\nnotify_lead_minutes = 25\nall_day_notify_hour = 7\n",
        )
        .unwrap();
        assert_eq!(settings.lead_minutes, 25);
        assert_eq!(settings.all_day_hour, 7);
    }

    #[test]
    fn a_nonsense_hour_is_refused_rather_than_silently_wrapped() {
        assert!(Settings::parse("all_day_notify_hour = 25").is_err());
    }

    #[test]
    fn a_partial_settings_file_keeps_the_defaults_for_the_rest() {
        let settings = Settings::parse("notify_lead_minutes = 30").unwrap();
        assert_eq!(settings.lead_minutes, 30);
        assert_eq!(settings.all_day_hour, DEFAULT_ALL_DAY_HOUR);
    }
}
