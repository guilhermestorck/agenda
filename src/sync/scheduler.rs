//! Keeping every account up to date, independently.
//!
//! Google Calendar has no push for desktop clients — a webhook needs a public HTTPS
//! endpoint — so this polls. `syncToken` makes that cheap: a quiet calendar returns an empty
//! delta.
//!
//! Accounts are synced one after another and their failures kept apart. SPEC §2.11: one
//! revoked token must park its own account on "Reconnect" and leave the others working.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use crate::auth::{self, RefreshRejected};
use crate::config::Credentials;
use crate::google::Session;
use crate::store::Store;

use super::{AccountReport, sync_account};

/// The floor: a change made in Google's UI shows up within a minute.
pub const MIN_INTERVAL: Duration = Duration::from_secs(60);
/// The ceiling, for a calendar where nothing has happened in a while.
pub const MAX_INTERVAL: Duration = Duration::from_secs(300);

/// How long to wait before the next poll.
///
/// Backs off while nothing is changing and snaps back the moment something does, so a busy
/// afternoon stays responsive and a quiet weekend stops asking. Doubling rather than
/// stepping, because the interesting range is only five times the floor.
pub fn next_interval(current: Duration, changed: bool) -> Duration {
    if changed {
        MIN_INTERVAL
    } else {
        (current * 2).min(MAX_INTERVAL).max(MIN_INTERVAL)
    }
}

/// What one pass over every account did.
#[derive(Debug, Default)]
pub struct Pass {
    /// Accounts Google has permanently rejected. These need the user, not another attempt.
    pub needs_reconnect: Vec<String>,
    pub stored: usize,
    pub deleted: usize,
    /// Accounts that failed for some other reason. Worth retrying, not worth reporting.
    pub transient_failures: Vec<String>,
}

impl Pass {
    pub fn changed(&self) -> bool {
        self.stored > 0 || self.deleted > 0
    }
}

/// Sync every connected account once.
///
/// Never returns `Err` for one account's sake: the whole point is that the others carry on.
pub async fn sync_all(store: Arc<Mutex<Store>>, credentials: Credentials) -> Pass {
    let accounts = match store.lock() {
        Ok(store) => store.accounts().unwrap_or_default(),
        Err(_) => {
            tracing::error!("the store lock was poisoned; skipping this pass");
            return Pass::default();
        }
    };

    let mut pass = Pass::default();
    for account in accounts {
        match sync_one(&store, &credentials, &account.email).await {
            Ok(report) => {
                pass.stored += report.stored;
                pass.deleted += report.deleted;
                if report.needs_reconnect() {
                    pass.needs_reconnect.push(account.email.clone());
                } else if !report.failures.is_empty() {
                    pass.transient_failures.push(account.email.clone());
                }
            }
            Err(error) => {
                if error.downcast_ref::<RefreshRejected>().is_some() {
                    tracing::info!(account = %account.email, "needs reconnecting");
                    pass.needs_reconnect.push(account.email.clone());
                } else {
                    tracing::warn!(
                        account = %account.email,
                        error = %format!("{error:#}"),
                        "an account failed to sync; the others are unaffected"
                    );
                    pass.transient_failures.push(account.email.clone());
                }
            }
        }
    }
    pass
}

async fn sync_one(
    store: &Arc<Mutex<Store>>,
    credentials: &Credentials,
    email: &str,
) -> Result<AccountReport> {
    let Some(tokens) = auth::keyring::load(email).await? else {
        // The row outlived its tokens. Reconnecting is the only way forward, and it is the
        // same affordance a revoked token gets.
        return Err(RefreshRejected {
            account: email.to_string(),
            reason: "no tokens are stored for this account".to_string(),
        }
        .into());
    };

    let before = tokens.access_token.clone();
    let mut session = Session::new(email, credentials.clone(), tokens);
    let report = sync_account(&mut session, store).await?;

    // A token refreshed mid-sync is only useful if it outlives the pass; otherwise every
    // run pays for a refresh it already did.
    if session.tokens.access_token != before {
        auth::keyring::store(email, &session.tokens).await?;
        tracing::debug!(account = %email, "stored a refreshed access token");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_that_changed_something_resets_to_the_floor() {
        assert_eq!(next_interval(MAX_INTERVAL, true), MIN_INTERVAL);
        assert_eq!(next_interval(MIN_INTERVAL, true), MIN_INTERVAL);
    }

    #[test]
    fn a_quiet_pass_backs_off_but_not_past_the_ceiling() {
        let mut interval = MIN_INTERVAL;
        for _ in 0..10 {
            interval = next_interval(interval, false);
            assert!(interval <= MAX_INTERVAL, "{interval:?} is past the ceiling");
        }
        assert_eq!(interval, MAX_INTERVAL);
    }

    #[test]
    fn backing_off_never_drops_below_the_floor() {
        assert!(next_interval(Duration::from_secs(1), false) >= MIN_INTERVAL);
    }

    #[test]
    fn a_pass_reports_change_only_when_something_moved() {
        assert!(!Pass::default().changed());
        assert!(
            Pass {
                stored: 1,
                ..Default::default()
            }
            .changed()
        );
        assert!(
            Pass {
                deleted: 1,
                ..Default::default()
            }
            .changed()
        );
        assert!(
            !Pass {
                transient_failures: vec!["a@b.com".to_string()],
                ..Default::default()
            }
            .changed(),
            "a failure is not a change; backing off on failure is the point"
        );
    }
}
