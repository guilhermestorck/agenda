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
/// Fill: the user's per-calendar override, then the account's colour, then Google's colour
/// for the calendar, then a generated fallback. Calendar beats account, because a choice
/// made about one calendar is more specific than a default set for all of them.
pub fn resolve(account: &Account, calendar: &Calendar) -> Colors {
    let marker = marker_for(account);
    let fill = calendar
        .user_color
        .clone()
        .or_else(|| account.color.clone())
        .or_else(|| calendar.color.clone())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn account(email: &str, color: Option<&str>) -> Account {
        Account {
            email: email.to_string(),
            label: None,
            color: color.map(str::to_string),
            sort_order: 0,
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
    fn the_users_calendar_override_wins_over_everything() {
        let colors = resolve(
            &account("work@example.com", Some("#e66100")),
            &calendar("work@example.com", "team", Some("#16a765"), Some("#ff0000")),
        );
        assert_eq!(colors.fill, "#ff0000");
    }

    #[test]
    fn without_an_override_the_account_colour_is_the_default_fill() {
        let colors = resolve(
            &account("work@example.com", Some("#e66100")),
            &calendar("work@example.com", "team", Some("#16a765"), None),
        );
        assert_eq!(colors.fill, "#e66100");
    }

    #[test]
    fn googles_colour_is_used_only_when_the_user_has_expressed_nothing() {
        let colors = resolve(
            &account("work@example.com", None),
            &calendar("work@example.com", "team", Some("#16a765"), None),
        );
        assert_eq!(colors.fill, "#16a765");
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
    fn setting_an_account_colour_moves_only_the_calendars_that_have_no_override() {
        let before = account("work@example.com", Some("#e66100"));
        let after = account("work@example.com", Some("#9141ac"));
        let plain = calendar("work@example.com", "a", None, None);
        let overridden = calendar("work@example.com", "b", None, Some("#ff0000"));

        assert_eq!(resolve(&before, &plain).fill, "#e66100");
        assert_eq!(resolve(&after, &plain).fill, "#9141ac");
        assert_eq!(resolve(&before, &overridden).fill, "#ff0000");
        assert_eq!(
            resolve(&after, &overridden).fill,
            "#ff0000",
            "a deliberate per-calendar choice must not move when the default does"
        );
    }
}
