//! Connecting accounts, and the colours that tell them apart.
//!
//! SPEC §1: there is no "current account". Connecting is additive — a second account joins
//! the first on the same grid, and nothing is ever hidden to make room for it.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::auth::{self, Redirect};
use crate::config::Credentials;
use crate::google::Session;
use crate::store::{CalendarMetadata, Store};

/// Marker colours, assigned in order as accounts are connected, so a newly connected
/// account is never unmarked (SPEC §4). Drawn from the Adwaita palette and ordered for
/// distinguishability rather than by hue, because the criterion that matters is telling two
/// accounts apart at a glance.
const PALETTE: [&str; 8] = [
    "#3584e4", // blue
    "#e66100", // orange
    "#2ec27e", // green
    "#9141ac", // purple
    "#e5a50a", // yellow
    "#c01c28", // red
    "#00b0c8", // teal
    "#865e3c", // brown
];

/// The colour a newly connected account gets. Wraps rather than running out; a ninth account
/// repeating the first's colour is worse than refusing to connect it, but only just, and the
/// user can override it.
pub fn palette_color(existing_accounts: usize) -> &'static str {
    PALETTE[existing_accounts % PALETTE.len()]
}

/// Which address the tokens belong to.
///
/// Taken from the primary calendar's id, which for a Google account *is* the account's email
/// address. The alternative is the userinfo endpoint, which needs an extra scope — and
/// widening a scope later forces every already-connected account to consent again.
fn account_email(calendars: &[CalendarMetadata]) -> Result<&str> {
    calendars
        .iter()
        .find(|calendar| calendar.is_primary)
        .map(|calendar| calendar.id.as_str())
        .context("the account has no primary calendar, so its address cannot be determined")
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// How long to wait for the browser before giving up, so a consent screen left open in a
/// forgotten tab does not hold a listener forever.
const CONSENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// What a connect attempt did, for the UI to report without parsing an error string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connected {
    Account {
        email: String,
        calendars: usize,
    },
    /// The user closed the consent screen or pressed Cancel. Not a failure to report as one.
    Declined,
}

/// Run the whole consent flow and record the result.
///
/// Ordered so that a failure at any point leaves nothing behind: tokens are obtained, then
/// the calendars are fetched with them, and only once both have succeeded is anything
/// written. The database write is a single transaction, and the keyring entry is rolled back
/// if it cannot be paired with one.
pub async fn connect(store: Arc<Mutex<Store>>, credentials: Credentials) -> Result<Connected> {
    // The lock is taken only around the synchronous database calls and never held across an
    // await: a std Mutex held over one would block a runtime thread for as long as the user
    // spends at the consent screen.
    let (pkce, state) = auth::begin()?;
    let loopback = auth::Loopback::bind()?;
    let url = auth::authorization_url(
        &credentials.client_id,
        &loopback.redirect_uri,
        &pkce,
        &state,
    )?;

    open_in_browser(&url)?;

    // tiny_http blocks, and blocking here would freeze the window for as long as the user
    // spends at the consent screen.
    let redirect = tokio::task::spawn_blocking(move || {
        loopback
            .wait_for_redirect(&state, CONSENT_TIMEOUT)
            .map(|redirect| (redirect, loopback.redirect_uri.clone()))
    })
    .await
    .context("the loopback listener panicked")??;

    let (redirect, redirect_uri) = redirect;
    let code = match redirect {
        Redirect::Code(code) => code,
        Redirect::Denied { reason } => {
            tracing::info!(%reason, "the user declined at the consent screen");
            return Ok(Connected::Declined);
        }
        Redirect::StateMismatch => bail!("the redirect did not match this sign-in attempt"),
        Redirect::Ignored => bail!("the loopback listener returned without an answer"),
    };

    let http = reqwest::Client::new();
    let tokens = auth::exchange_code(
        &http,
        &credentials.client_id,
        &credentials.client_secret,
        &code,
        &pkce,
        &redirect_uri,
    )
    .await?;

    // The address is not known until the calendars are in hand, which is also what makes
    // this the right point to fail: nothing has been written yet.
    let mut session = Session::new("(connecting)", credentials.clone(), tokens.clone());
    let calendars = session.calendars().await?;
    let email = account_email(&calendars)?.to_string();
    let calendars: Vec<CalendarMetadata> = calendars
        .into_iter()
        .map(|calendar| CalendarMetadata {
            account: email.clone(),
            ..calendar
        })
        .collect();

    {
        let mut store = store.lock().expect("the store lock was poisoned");
        let color = palette_color(store.account_count()?);
        store.connect_account(&email, color, now_unix(), &calendars)?;
    }

    if let Err(error) = auth::keyring::store(&email, &session.tokens).await {
        // An account the app can see but cannot authenticate would sit in the sidebar
        // looking connected and never sync.
        store
            .lock()
            .expect("the store lock was poisoned")
            .remove_account(&email)?;
        return Err(error.context("the account was not connected"));
    }

    tracing::info!(%email, calendars = calendars.len(), "connected an account");
    Ok(Connected::Account {
        email,
        calendars: calendars.len(),
    })
}

/// Hand the URL to the desktop rather than picking a browser. `xdg-open` is what every
/// other desktop application uses, and it honours whatever the user has set as default.
fn open_in_browser(url: &str) -> Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .context("could not open a browser; is xdg-utils installed?")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calendar(account: &str, id: &str, is_primary: bool) -> CalendarMetadata {
        CalendarMetadata {
            account: account.to_string(),
            id: id.to_string(),
            summary: id.to_string(),
            color: None,
            timezone: None,
            access_role: "owner".to_string(),
            is_primary,
        }
    }

    #[test]
    fn the_first_accounts_each_get_a_different_marker_colour() {
        // Criterion 4 is that two accounts are distinguishable at a glance; handing the
        // second account the first's colour loses that before the user does anything.
        let assigned: Vec<&str> = (0..PALETTE.len()).map(palette_color).collect();
        let unique: std::collections::HashSet<_> = assigned.iter().collect();
        assert_eq!(unique.len(), PALETTE.len());
    }

    #[test]
    fn a_ninth_account_wraps_rather_than_going_unmarked() {
        assert_eq!(palette_color(PALETTE.len()), palette_color(0));
        assert!(!palette_color(100).is_empty());
    }

    #[test]
    fn the_account_address_is_taken_from_its_primary_calendar() {
        let calendars = [
            calendar("?", "es.spanish#holiday@group.v.calendar.google.com", false),
            calendar("?", "work@example.com", true),
        ];
        assert_eq!(account_email(&calendars).unwrap(), "work@example.com");
    }

    #[test]
    fn an_account_with_no_primary_calendar_fails_before_anything_is_written() {
        let calendars = [calendar("?", "c_shared@group.calendar.google.com", false)];
        assert!(account_email(&calendars).is_err());
    }

    #[test]
    fn connecting_an_account_records_it_with_all_of_its_calendars() {
        let mut store = Store::open_in_memory().unwrap();
        let calendars = [
            calendar("work@example.com", "work@example.com", true),
            calendar("work@example.com", "team", false),
        ];
        store
            .connect_account("work@example.com", "#3584e4", 1_700_000_000, &calendars)
            .unwrap();

        assert_eq!(store.account_count().unwrap(), 1);
        assert!(
            store
                .calendar("work@example.com", "team")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_connect_that_fails_part_way_leaves_no_partial_account() {
        // The second calendar names an account that does not exist, so its foreign key
        // fails. Without a transaction the account row and the first calendar would survive
        // as an account that can never sync.
        let mut store = Store::open_in_memory().unwrap();
        let calendars = [
            calendar("work@example.com", "work@example.com", true),
            calendar("somebody-else@example.com", "team", false),
        ];
        store
            .connect_account("work@example.com", "#3584e4", 1_700_000_000, &calendars)
            .expect_err("the foreign key must fail the whole transaction");

        assert_eq!(
            store.account_count().unwrap(),
            0,
            "SPEC §2: a failed connect leaves no partial accounts row"
        );
        assert!(
            store
                .calendar("work@example.com", "work@example.com")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn connecting_a_second_account_adds_to_the_first_rather_than_replacing_it() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .connect_account(
                "personal@example.com",
                palette_color(0),
                1,
                &[calendar(
                    "personal@example.com",
                    "personal@example.com",
                    true,
                )],
            )
            .unwrap();
        store
            .connect_account(
                "work@example.com",
                palette_color(1),
                2,
                &[calendar("work@example.com", "work@example.com", true)],
            )
            .unwrap();

        assert_eq!(
            store.account_count().unwrap(),
            2,
            "there is no account switcher"
        );
        assert!(
            store
                .calendar("personal@example.com", "personal@example.com")
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .calendar("work@example.com", "work@example.com")
                .unwrap()
                .is_some()
        );
    }
}
