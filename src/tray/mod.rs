//! The StatusNotifierItem, and what it says.
//!
//! KDE Plasma speaks StatusNotifierItem natively, so no AppIndicator shim is needed. The
//! item shows the next thing on the calendar and toggles the window; **no quick-add**, which
//! is a write, and the write path is deferred past v1 (SPEC §2).

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;

use crate::recur::Occurrence;

/// What the tray should say right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Something is on at this moment. More useful than what comes after it.
    InProgress {
        summary: String,
        ends_in: i64,
    },
    Upcoming {
        summary: String,
        starts_in: i64,
    },
    Nothing,
}

impl Next {
    /// One line, short enough for a tooltip and a menu header.
    pub fn describe(&self, zone: Tz, start_utc: Option<i64>) -> String {
        match self {
            Next::Nothing => "Nothing scheduled".to_string(),
            Next::InProgress { summary, ends_in } => {
                format!("{summary} — ends in {}", humanise(*ends_in))
            }
            Next::Upcoming { summary, starts_in } => match start_utc {
                Some(start) if *starts_in > 12 * 3600 => {
                    format!("{summary} — {}", at_time(start, zone))
                }
                _ => format!("{summary} — in {}", humanise(*starts_in)),
            },
        }
    }
}

fn at_time(start_utc: i64, zone: Tz) -> String {
    Utc.timestamp_opt(start_utc, 0)
        .single()
        .map(|instant| instant.with_timezone(&zone).format("%a %H:%M").to_string())
        .unwrap_or_default()
}

/// Whole minutes below an hour, whole hours above it. Nobody needs "in 47 minutes and 12
/// seconds", and a tray that counts seconds is a tray that redraws every second.
fn humanise(seconds: i64) -> String {
    let minutes = (seconds + 59) / 60;
    match minutes {
        ..=0 => "less than a minute".to_string(),
        1 => "1 minute".to_string(),
        2..=59 => format!("{minutes} minutes"),
        60..=119 => "1 hour".to_string(),
        _ => format!("{} hours", minutes / 60),
    }
}

/// The single most relevant occurrence at `now`.
///
/// An event already under way beats one that has not started: "in 20 minutes" is no use when
/// the meeting you are late for is on screen behind it. All-day events are skipped — one
/// that lasts all day is not a thing you are about to be late for, and it would otherwise
/// mask every timed event on that day.
pub fn next_event(occurrences: &[Occurrence], now: i64) -> Next {
    let timed = || occurrences.iter().filter(|o| !o.event.all_day);

    if let Some(current) = timed()
        .filter(|o| o.start_utc <= now && o.end_utc > now)
        .min_by_key(|o| o.end_utc)
    {
        return Next::InProgress {
            summary: title_of(current),
            ends_in: current.end_utc - now,
        };
    }

    match timed()
        .filter(|o| o.start_utc > now)
        .min_by_key(|o| o.start_utc)
    {
        Some(next) => Next::Upcoming {
            summary: title_of(next),
            starts_in: next.start_utc - now,
        },
        None => Next::Nothing,
    }
}

/// When the tray should next redraw: the moment the answer changes.
pub fn next_change(occurrences: &[Occurrence], now: i64) -> Option<i64> {
    occurrences
        .iter()
        .filter(|o| !o.event.all_day)
        .flat_map(|o| [o.start_utc, o.end_utc])
        .filter(|instant| *instant > now)
        .min()
}

fn title_of(occurrence: &Occurrence) -> String {
    if occurrence.event.summary.is_empty() {
        "(no title)".to_string()
    } else {
        occurrence.event.summary.clone()
    }
}

/// The start of whatever `next_event` picked, for `describe`.
pub fn next_start(occurrences: &[Occurrence], now: i64) -> Option<i64> {
    occurrences
        .iter()
        .filter(|o| !o.event.all_day && o.start_utc > now)
        .map(|o| o.start_utc)
        .min()
}

/// What the tray asks the window to do. The tray runs on its own thread and must never
/// touch a widget; it posts one of these instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    ToggleWindow,
    Quit,
}

/// The StatusNotifierItem itself.
pub struct Item {
    line: String,
    requests: std::sync::mpsc::Sender<Request>,
}

impl Item {
    pub fn set_line(&mut self, line: String) {
        self.line = line;
    }
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "io.github.guilhermestorck.agenda".to_string()
    }

    fn icon_name(&self) -> String {
        "x-office-calendar-symbolic".to_string()
    }

    fn title(&self) -> String {
        "agenda".to_string()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "agenda".to_string(),
            description: self.line.clone(),
            icon_name: self.icon_name(),
            icon_pixmap: Vec::new(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.requests.send(Request::ToggleWindow);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        vec![
            // The next event as a disabled line rather than a tooltip alone: a tooltip needs
            // hovering, and the whole point is to answer the question without interaction.
            StandardItem {
                label: self.line.clone(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Show agenda".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.requests.send(Request::ToggleWindow);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.requests.send(Request::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
        // No quick-add. It is a write, and the write path is deferred past v1 (SPEC §2).
    }
}

/// Start the tray. Returns the handle used to update its text and the receiver the window
/// polls for requests.
///
/// ksni re-registers itself with the host, so a `plasmashell` restart brings the item back
/// without anything here noticing.
pub fn start(
    line: String,
) -> anyhow::Result<(
    ksni::blocking::Handle<Item>,
    std::sync::mpsc::Receiver<Request>,
)> {
    use ksni::blocking::TrayMethods;

    let (requests, incoming) = std::sync::mpsc::channel();
    let handle = Item { line, requests }
        .spawn()
        .map_err(|error| anyhow::anyhow!("could not register a tray item: {error}"))?;
    Ok((handle, incoming))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Event;

    const MADRID: Tz = chrono_tz::Europe::Madrid;

    fn occurrence(account: &str, summary: &str, start: i64, end: i64, all_day: bool) -> Occurrence {
        Occurrence {
            start_utc: start,
            end_utc: end,
            event: Event {
                account: account.to_string(),
                calendar_id: "primary".to_string(),
                id: summary.to_string(),
                ical_uid: None,
                etag: None,
                summary: summary.to_string(),
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
                updated_at: None,
            },
        }
    }

    /// Two accounts, because the tray reports across all of them at once (SPEC §8).
    fn both_accounts() -> Vec<Occurrence> {
        vec![
            occurrence("work@example.com", "Standup", 1_000, 1_900, false),
            occurrence("personal@example.com", "Dentist", 5_000, 8_600, false),
        ]
    }

    #[test]
    fn the_next_event_can_come_from_any_account() {
        // There is no current account; the tray reports whatever is nearest across all.
        let next = next_event(&both_accounts(), 2_000);
        assert_eq!(
            next,
            Next::Upcoming {
                summary: "Dentist".to_string(),
                starts_in: 3_000
            }
        );
    }

    #[test]
    fn an_event_already_under_way_beats_one_that_has_not_started() {
        // "In 20 minutes" is no use when the meeting you are late for is on screen behind it.
        let next = next_event(&both_accounts(), 1_500);
        assert_eq!(
            next,
            Next::InProgress {
                summary: "Standup".to_string(),
                ends_in: 400
            }
        );
    }

    #[test]
    fn an_event_that_has_ended_is_not_the_next_one() {
        let next = next_event(&both_accounts(), 9_000);
        assert_eq!(next, Next::Nothing);
    }

    #[test]
    fn an_all_day_event_does_not_mask_the_timed_events_of_its_day() {
        // An all-day event lasts all day, so it would otherwise be "in progress" from
        // midnight and hide every meeting on it.
        let mut occurrences = both_accounts();
        occurrences.push(occurrence("work@example.com", "Holiday", 0, 86_400, true));
        assert!(matches!(
            next_event(&occurrences, 1_500),
            Next::InProgress { ref summary, .. } if summary == "Standup"
        ));
    }

    #[test]
    fn nothing_scheduled_is_a_state_and_not_an_error() {
        assert_eq!(next_event(&[], 1_000), Next::Nothing);
        assert_eq!(Next::Nothing.describe(MADRID, None), "Nothing scheduled");
    }

    #[test]
    fn two_events_in_progress_report_the_one_that_ends_first() {
        let occurrences = vec![
            occurrence("a@example.com", "Long", 0, 10_000, false),
            occurrence("b@example.com", "Short", 0, 2_000, false),
        ];
        assert!(matches!(
            next_event(&occurrences, 500),
            Next::InProgress { ref summary, .. } if summary == "Short"
        ));
    }

    #[test]
    fn an_event_without_a_title_still_says_something() {
        let occurrences = vec![occurrence("a@example.com", "", 1_000, 2_000, false)];
        assert!(matches!(
            next_event(&occurrences, 0),
            Next::Upcoming { ref summary, .. } if summary == "(no title)"
        ));
    }

    #[test]
    fn the_tray_redraws_when_the_answer_changes_and_not_before() {
        // A tray that counts seconds is a tray that redraws every second.
        let occurrences = both_accounts();
        assert_eq!(next_change(&occurrences, 0), Some(1_000), "the first start");
        assert_eq!(next_change(&occurrences, 1_200), Some(1_900), "its end");
        assert_eq!(
            next_change(&occurrences, 8_600),
            None,
            "nothing left to change"
        );
    }

    #[test]
    fn a_soon_event_counts_down_and_a_distant_one_gives_a_time() {
        let soon = Next::Upcoming {
            summary: "Standup".to_string(),
            starts_in: 1_800,
        };
        assert_eq!(
            soon.describe(MADRID, Some(1_789_371_000)),
            "Standup — in 30 minutes"
        );

        let distant = Next::Upcoming {
            summary: "Review".to_string(),
            starts_in: 90_000,
        };
        assert!(distant.describe(MADRID, Some(1_789_371_000)).contains(':'));
    }

    #[test]
    fn durations_are_rounded_to_something_worth_reading() {
        assert_eq!(humanise(30), "1 minute");
        assert_eq!(humanise(60), "1 minute");
        assert_eq!(humanise(1_800), "30 minutes");
        assert_eq!(humanise(3_600), "1 hour");
        assert_eq!(humanise(7_200), "2 hours");
        assert_eq!(humanise(0), "less than a minute");
    }
}
