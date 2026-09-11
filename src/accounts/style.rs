//! Which colour an event is drawn in, and which colour says whose it is.
//!
//! SPEC §1 keeps two dimensions independently readable: an event's **fill** is its
//! calendar's, and its **marker** is its account's. The marker is never the calendar's —
//! that is the whole reason a week with two accounts stays legible when their Google colours
//! happen to be similar.

use crate::store::{Account, Calendar};

use super::PALETTE;

/// The two colours a week-grid widget needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colors {
    /// The event body.
    pub fill: String,
    /// The account cue, always the account's own colour.
    pub marker: String,
}

/// Resolve both, per SPEC §4.
///
/// Fill: the user's per-calendar override, then Google's colour for the calendar, then the
/// account's colour, then a generated fallback.
///
/// The account's colour sits *below* Google's deliberately, and §4 was corrected on
/// 2026-09-11 to say so. Above it, an account colour — which every account is assigned on
/// connect — became the fill of every one of its calendars, so fill and marker were the same
/// colour and the two cues collapsed into one. That is precisely what §1 keeps apart: the
/// fill says which calendar, the marker says which account, and they have to stay readable
/// independently.
pub fn resolve(account: &Account, calendar: &Calendar) -> Colors {
    let marker = marker_for(account);
    let fill = calendar
        .user_color
        .clone()
        .or_else(|| calendar.color.clone())
        .or_else(|| account.color.clone())
        .unwrap_or_else(|| generated(&format!("{}/{}", calendar.account, calendar.id)));
    Colors { fill, marker }
}

/// The account's marker. Assigned from a palette on connect, so this should never have to
/// fall back — but an account whose colour was somehow lost is better marked arbitrarily
/// than not at all.
pub fn marker_for(account: &Account) -> String {
    account
        .color
        .clone()
        .unwrap_or_else(|| generated(&account.email))
}

/// A stable colour for something that has none.
///
/// Hashed with FNV-1a rather than `DefaultHasher`, whose output is explicitly not stable
/// across Rust releases — a rebuild silently recolouring the user's calendars would look
/// like a bug in the styling they set.
fn generated(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    PALETTE[(hash % PALETTE.len() as u64) as usize].to_string()
}

/// Black or white, whichever actually contrasts better against `fill`.
///
/// Computed rather than thresholded. A guessed cutoff put white on the palette's blue at
/// 3.77:1 — under the WCAG AA floor — when black would have given 5.57:1. Uses relative
/// luminance rather than a naive average, because a saturated green and a saturated blue of
/// the same average brightness are nothing alike to read against, and the palette has both.
pub fn text_on(fill: &str) -> &'static str {
    // An unreadable colour string must not make an event invisible; white on the Adwaita
    // accent is the safe default.
    let Some(fill) = luminance(fill) else {
        return "#ffffff";
    };
    let against_white = (1.0 + 0.05) / (fill + 0.05);
    let against_black = (fill + 0.05) / 0.05;
    if against_black >= against_white {
        "#000000"
    } else {
        "#ffffff"
    }
}

fn luminance(color: &str) -> Option<f64> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |offset: usize| -> Option<f64> {
        let value = u8::from_str_radix(&hex[offset..offset + 2], 16).ok()?;
        let value = f64::from(value) / 255.0;
        Some(if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        })
    };
    Some(0.2126 * channel(0)? + 0.7152 * channel(2)? + 0.0722 * channel(4)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(email: &str, color: Option<&str>) -> Account {
        Account {
            email: email.to_string(),
            label: None,
            color: color.map(str::to_string),
            sort_order: 0,
            display_name: None,
            picture_url: None,
        }
    }

    fn calendar(account: &str, id: &str, google: Option<&str>, user: Option<&str>) -> Calendar {
        Calendar {
            account: account.to_string(),
            id: id.to_string(),
            summary: id.to_string(),
            color: google.map(str::to_string),
            timezone: None,
            access_role: "owner".to_string(),
            is_primary: false,
            visible: true,
            user_color: user.map(str::to_string),
            sync_token: None,
            synced_at: None,
        }
    }

    #[test]
    fn text_stays_legible_against_every_palette_colour() {
        // Criterion: an event whose title cannot be read is an event that is not on screen.
        for fill in PALETTE {
            let text = text_on(fill);
            let contrast = contrast_ratio(fill, text);
            assert!(
                contrast >= 4.5,
                "{fill} on {text} is {contrast:.2}:1, below the 4.5:1 readability floor"
            );
        }
    }

    #[test]
    fn a_light_fill_takes_dark_text_and_a_dark_fill_takes_light() {
        assert_eq!(text_on("#ffffff"), "#000000");
        assert_eq!(text_on("#000000"), "#ffffff");
        assert_eq!(text_on("#f6f5f4"), "#000000");
    }

    #[test]
    fn an_unreadable_colour_string_still_yields_usable_text() {
        // A malformed user colour must not make an event invisible.
        assert_eq!(text_on("not a colour"), "#ffffff");
        assert_eq!(text_on("#fff"), "#ffffff");
    }

    fn contrast_ratio(a: &str, b: &str) -> f64 {
        let first = luminance(a).unwrap();
        let second = luminance(b).unwrap();
        let (lighter, darker) = if first > second {
            (first, second)
        } else {
            (second, first)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn the_users_calendar_override_wins_over_everything() {
        let colors = resolve(
            &account("work@example.com", Some("#e66100")),
            &calendar("work@example.com", "team", Some("#16a765"), Some("#ff0000")),
        );
        assert_eq!(colors.fill, "#ff0000");
    }

    #[test]
    fn googles_calendar_colour_is_the_fill_when_the_user_has_set_no_override() {
        // The account has a colour — every account does, assigned on connect — and it must
        // not swallow the calendar's. Above Google's in the chain, every event on the
        // account ends up one flat colour and the calendar dimension disappears.
        let colors = resolve(
            &account("work@example.com", Some("#e66100")),
            &calendar("work@example.com", "team", Some("#16a765"), None),
        );
        assert_eq!(colors.fill, "#16a765");
        assert_eq!(
            colors.marker, "#e66100",
            "and the marker is still the account's"
        );
    }

    #[test]
    fn the_account_colour_fills_a_calendar_google_gave_none() {
        let colors = resolve(
            &account("work@example.com", Some("#e66100")),
            &calendar("work@example.com", "team", None, None),
        );
        assert_eq!(colors.fill, "#e66100");
    }

    #[test]
    fn a_calendar_with_no_colour_at_all_still_gets_one() {
        let colors = resolve(
            &account("work@example.com", None),
            &calendar("work@example.com", "team", None, None),
        );
        assert!(colors.fill.starts_with('#'));
        assert_eq!(colors.fill.len(), 7);
    }

    #[test]
    fn the_generated_fallback_is_the_same_on_every_run() {
        // DefaultHasher is not stable across Rust releases; a rebuild recolouring the user's
        // calendars would look like a bug in the styling they set.
        assert_eq!(
            generated("work@example.com/team"),
            generated("work@example.com/team")
        );
        assert_ne!(
            generated("work@example.com/team"),
            generated("work@example.com/other")
        );
    }

    #[test]
    fn the_marker_is_the_accounts_colour_and_never_the_calendars() {
        // Criterion 4: the case this must survive is two accounts whose calendars carry
        // similar Google colours. If the marker followed the calendar it would carry no
        // account information at all.
        let personal = account("personal@example.com", Some("#3584e4"));
        let work = account("work@example.com", Some("#e66100"));
        let similar_a = calendar("personal@example.com", "primary", Some("#16a765"), None);
        let similar_b = calendar("work@example.com", "primary", Some("#16a766"), None);

        let first = resolve(&personal, &similar_a);
        let second = resolve(&work, &similar_b);
        assert_ne!(
            first.marker, second.marker,
            "two accounts must stay distinguishable however alike their calendars look"
        );
        assert_eq!(first.marker, "#3584e4");
        assert_eq!(second.marker, "#e66100");
    }

    #[test]
    fn an_override_does_not_change_the_marker() {
        let account = account("work@example.com", Some("#e66100"));
        let plain = resolve(&account, &calendar("work@example.com", "a", None, None));
        let overridden = resolve(
            &account,
            &calendar("work@example.com", "b", None, Some("#ff0000")),
        );
        assert_eq!(plain.marker, overridden.marker);
        assert_ne!(plain.fill, overridden.fill);
    }

    #[test]
    fn setting_an_account_colour_moves_only_what_has_no_colour_of_its_own() {
        let before = account("work@example.com", Some("#e66100"));
        let after = account("work@example.com", Some("#9141ac"));
        let colourless = calendar("work@example.com", "a", None, None);
        let from_google = calendar("work@example.com", "b", Some("#16a765"), None);
        let overridden = calendar("work@example.com", "c", None, Some("#ff0000"));

        assert_eq!(resolve(&before, &colourless).fill, "#e66100");
        assert_eq!(resolve(&after, &colourless).fill, "#9141ac");
        assert_eq!(
            resolve(&after, &from_google).fill,
            "#16a765",
            "a calendar with its own colour keeps it"
        );
        assert_eq!(
            resolve(&after, &overridden).fill,
            "#ff0000",
            "a deliberate per-calendar choice must not move when the default does"
        );
    }

    #[test]
    fn the_fill_and_the_marker_stay_different_colours_by_default() {
        // The regression this file exists to prevent: fill and marker collapsing into one,
        // which makes the stripe invisible and throws away the calendar dimension.
        let account = account("work@example.com", Some("#e66100"));
        let colors = resolve(
            &account,
            &calendar("work@example.com", "team", Some("#16a765"), None),
        );
        assert_ne!(colors.fill, colors.marker);
    }
}
