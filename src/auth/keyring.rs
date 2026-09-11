//! Tokens in the Secret Service, never on disk.
//!
//! Keyed by account email, because several accounts coexist: SPEC §1 is explicit that there
//! is no "current account", so the keyring holds one entry per connected address and one
//! account's revoked token must not disturb the others.
//!
//! On this machine the Secret Service is KDE's ksecretd rather than GNOME Keyring, which was
//! the main stack risk — oo7 is written against the latter. The round-trip test below is the
//! thing that proves it, and it is `#[ignore]`d so a plain `cargo test` never writes to the
//! user's real keyring.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The attribute every entry carries, so `search_items` can find ours and only ours.
const APPLICATION: &str = "agenda";

/// What Google hands back at the token endpoint.
///
/// Not `derive(Debug)`: a refresh token in a log is a durable credential in a log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    /// Absent when Google declines to reissue one on a refresh, which it does whenever the
    /// existing refresh token is still good.
    pub refresh_token: Option<String>,
    /// Unix seconds. Stored rather than a duration so a token loaded from the keyring after
    /// a restart is judged against the clock, not against when the process started.
    pub expires_at: i64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Tokens {
    /// Treated as expired a minute early, so a token is never sent to Google in the moment
    /// between passing this check and arriving at the API.
    pub fn is_expired_at(&self, now: i64) -> bool {
        now >= self.expires_at - 60
    }
}

fn attributes(account: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", APPLICATION), ("account", account)])
}

pub async fn store(account: &str, tokens: &Tokens) -> Result<()> {
    let keyring = oo7::Keyring::new()
        .await
        .context("could not reach the Secret Service")?;
    let secret = serde_json::to_vec(tokens).context("could not serialise the tokens")?;
    keyring
        .create_item(
            &format!("agenda — {account}"),
            &attributes(account),
            &secret,
            true,
        )
        .await
        .with_context(|| format!("could not store the tokens for {account}"))?;
    Ok(())
}

pub async fn load(account: &str) -> Result<Option<Tokens>> {
    let keyring = oo7::Keyring::new()
        .await
        .context("could not reach the Secret Service")?;
    let items = keyring
        .search_items(&attributes(account))
        .await
        .with_context(|| format!("could not search the keyring for {account}"))?;

    let Some(item) = items.first() else {
        return Ok(None);
    };
    let secret = item
        .secret()
        .await
        .with_context(|| format!("could not unlock the tokens for {account}"))?;
    let tokens = serde_json::from_slice(&secret)
        .with_context(|| format!("the stored tokens for {account} are not readable"))?;
    Ok(Some(tokens))
}

/// Every account with tokens in the keyring. This is what tells the app which accounts to
/// sync on a cold start, before anything has touched the database.
pub async fn accounts() -> Result<Vec<String>> {
    let keyring = oo7::Keyring::new()
        .await
        .context("could not reach the Secret Service")?;
    let items = keyring
        .search_items(&HashMap::from([("application", APPLICATION)]))
        .await
        .context("could not list the keyring entries")?;

    let mut found = Vec::new();
    for item in items {
        let attributes = item.attributes().await.context("could not read an entry")?;
        if let Some(account) = attributes.get("account") {
            found.push(account.to_string());
        }
    }
    found.sort();
    Ok(found)
}

pub async fn delete(account: &str) -> Result<()> {
    let keyring = oo7::Keyring::new()
        .await
        .context("could not reach the Secret Service")?;
    keyring
        .delete(&attributes(account))
        .await
        .with_context(|| format!("could not remove the tokens for {account}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(access: &str, expires_at: i64) -> Tokens {
        Tokens {
            access_token: access.to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at,
        }
    }

    #[test]
    fn a_token_is_treated_as_expired_a_minute_before_it_actually_is() {
        let tokens = tokens("a", 1_000);
        assert!(!tokens.is_expired_at(930));
        assert!(
            tokens.is_expired_at(940),
            "the skew margin must start at 60s"
        );
        assert!(tokens.is_expired_at(1_000));
    }

    #[test]
    fn the_debug_impl_never_prints_a_token() {
        let rendered = format!("{:?}", tokens("super-secret", 1_000));
        assert!(!rendered.contains("super-secret"));
        assert!(
            !rendered.contains("refresh\""),
            "the refresh token is the durable one"
        );
    }

    #[test]
    fn tokens_survive_the_json_round_trip_that_the_keyring_stores() {
        let original = tokens("access", 1_700_000_000);
        let encoded = serde_json::to_vec(&original).unwrap();
        let decoded: Tokens = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[tokio::test]
    #[ignore = "writes to the user's real keyring; run deliberately with `cargo test -- --ignored`"]
    async fn two_accounts_round_trip_through_the_secret_service_independently() {
        // The stack risk this proves: oo7 is written against GNOME Keyring, and KDE's
        // Secret Service is a different daemon (ksecretd). Two accounts, because a
        // single-account test passes trivially for account-keyed storage.
        let first = "agenda-test-first@example.com";
        let second = "agenda-test-second@example.com";

        store(first, &tokens("first-access", 111)).await.unwrap();
        store(second, &tokens("second-access", 222)).await.unwrap();

        assert_eq!(
            load(first).await.unwrap().unwrap().access_token,
            "first-access"
        );
        assert_eq!(
            load(second).await.unwrap().unwrap().access_token,
            "second-access"
        );

        let listed = accounts().await.unwrap();
        assert!(listed.contains(&first.to_string()));
        assert!(listed.contains(&second.to_string()));

        // Deleting one must not disturb the other — SPEC §2.11 in miniature.
        delete(first).await.unwrap();
        assert!(load(first).await.unwrap().is_none());
        assert_eq!(
            load(second).await.unwrap().unwrap().access_token,
            "second-access"
        );

        delete(second).await.unwrap();
        assert!(load(second).await.unwrap().is_none());
    }
}
