//! PKCE (RFC 7636) and the CSRF `state`.
//!
//! A desktop client cannot keep a secret, so the authorization code is bound to a one-time
//! verifier instead: the challenge goes out with the consent request, the verifier comes
//! back with the code exchange, and an intercepted code is worthless without it.

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// One attempt's verifier and the challenge derived from it.
pub struct Pkce {
    /// Sent only at the token exchange, never in a URL.
    pub verifier: String,
    /// Sent in the authorization URL, as `code_challenge` with method `S256`.
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Result<Self> {
        let verifier = random_token()?;
        let challenge = challenge_for(&verifier);
        Ok(Self {
            verifier,
            challenge,
        })
    }
}

/// 32 bytes of OS randomness as unpadded base64url — 43 characters, entirely within the
/// unreserved set RFC 7636 requires of a verifier, so it is also the right shape for the
/// CSRF `state`.
pub fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).context("the OS refused to supply random bytes")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The `S256` transformation. `plain` is also permitted by the RFC, but it puts the
/// verifier itself in the authorization URL, where anything watching the loopback redirect
/// would see it.
fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rfc_7636_appendix_b_vector_produces_its_published_challenge() {
        // The published example from the RFC itself, rather than a value this code
        // generated and then froze — which would only prove it is self-consistent.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_generated_verifier_is_within_the_length_the_rfc_allows() {
        let pkce = Pkce::generate().unwrap();
        assert!(
            (43..=128).contains(&pkce.verifier.len()),
            "RFC 7636 §4.1 requires 43 to 128 characters, got {}",
            pkce.verifier.len()
        );
    }

    #[test]
    fn a_generated_verifier_uses_only_the_unreserved_characters() {
        let pkce = Pkce::generate().unwrap();
        assert!(
            pkce.verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)),
            "a verifier outside the unreserved set is rejected at the token endpoint: {}",
            pkce.verifier
        );
    }

    #[test]
    fn the_challenge_carries_no_base64_padding() {
        // '=' is percent-encoded in a URL and Google compares the raw string; padding here
        // fails the exchange with an opaque invalid_grant.
        assert!(!Pkce::generate().unwrap().challenge.contains('='));
    }

    #[test]
    fn two_attempts_never_share_a_verifier() {
        // A reused verifier would let a code intercepted from an earlier attempt be
        // exchanged against this one.
        let first = Pkce::generate().unwrap();
        let second = Pkce::generate().unwrap();
        assert_ne!(first.verifier, second.verifier);
        assert_ne!(first.challenge, second.challenge);
    }

    #[test]
    fn two_states_never_collide() {
        assert_ne!(random_token().unwrap(), random_token().unwrap());
    }
}
