//! Reading Google's answer off the loopback redirect.
//!
//! The consent screen sends the browser to `http://127.0.0.1:<port>/?code=…&state=…`. What
//! arrives on that socket is not only that request: browsers ask for `/favicon.ico`
//! unprompted, and a listener that treats the first request as the answer will take the
//! favicon for a failed login and give up while the real redirect is still in flight.

/// What a single request to the loopback listener turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Redirect {
    /// The authorization code. Keep it; stop listening.
    Code(String),
    /// The user said no at the consent screen. Not an error in the system sense — nothing
    /// is broken and no retry helps — but the flow is over.
    Denied { reason: String },
    /// Noise. Keep listening.
    Ignored,
    /// The `state` did not match the one we sent. Someone else's redirect, or a forgery.
    StateMismatch,
}

/// Parse one request path against the `state` we generated for this attempt.
pub fn parse_redirect(path: &str, expected_state: &str) -> Redirect {
    let Some((_, query)) = path.split_once('?') else {
        return Redirect::Ignored;
    };

    let mut code = None;
    let mut error = None;
    let mut state = None;
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        match key {
            "code" => code = Some(percent_decode(value)),
            "error" => error = Some(percent_decode(value)),
            "state" => state = Some(percent_decode(value)),
            _ => {}
        }
    }

    if code.is_none() && error.is_none() {
        return Redirect::Ignored;
    }

    // Checked before the code is looked at, so a forged redirect never reaches the token
    // endpoint. Compared even on the error path: an attacker-supplied `error` would
    // otherwise be able to abort a legitimate login.
    if state.as_deref() != Some(expected_state) {
        return Redirect::StateMismatch;
    }

    match (code, error) {
        (_, Some(reason)) => Redirect::Denied { reason },
        (Some(code), None) => Redirect::Code(code),
        (None, None) => Redirect::Ignored,
    }
}

/// Google's authorization codes contain `/`, which browsers percent-encode. Decoding is not
/// optional: a code with a literal `%2F` left in it is rejected at the token endpoint.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&raw[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    // Not a valid escape; a literal '%' is likelier than a lost byte.
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "the-state-we-sent";

    #[test]
    fn the_authorization_code_is_read_from_a_matching_redirect() {
        let redirect = parse_redirect(&format!("/?code=4%2F0Aeanabc&state={STATE}"), STATE);
        assert_eq!(redirect, Redirect::Code("4/0Aeanabc".to_string()));
    }

    #[test]
    fn a_code_containing_a_slash_is_percent_decoded() {
        // Left encoded, the token endpoint rejects it and the failure is opaque.
        let redirect = parse_redirect(&format!("/?code=4%2F0A%2Fb&state={STATE}"), STATE);
        assert_eq!(redirect, Redirect::Code("4/0A/b".to_string()));
    }

    #[test]
    fn the_browsers_favicon_request_is_ignored_rather_than_taken_for_a_failure() {
        assert_eq!(parse_redirect("/favicon.ico", STATE), Redirect::Ignored);
    }

    #[test]
    fn a_bare_request_with_no_query_is_ignored() {
        assert_eq!(parse_redirect("/", STATE), Redirect::Ignored);
    }

    #[test]
    fn a_query_carrying_neither_code_nor_error_is_ignored() {
        assert_eq!(
            parse_redirect("/?utm_source=whatever", STATE),
            Redirect::Ignored
        );
    }

    #[test]
    fn a_redirect_whose_state_does_not_match_is_refused() {
        let redirect = parse_redirect("/?code=4%2F0Aeanabc&state=somebody-elses", STATE);
        assert_eq!(redirect, Redirect::StateMismatch);
    }

    #[test]
    fn a_redirect_with_no_state_at_all_is_refused() {
        assert_eq!(
            parse_redirect("/?code=4%2F0Aeanabc", STATE),
            Redirect::StateMismatch
        );
    }

    #[test]
    fn an_error_with_a_forged_state_cannot_abort_a_legitimate_login() {
        // The state check runs before the error is honoured, or anyone able to reach the
        // loopback port could cancel the user's sign-in.
        let redirect = parse_redirect("/?error=access_denied&state=somebody-elses", STATE);
        assert_eq!(redirect, Redirect::StateMismatch);
    }

    #[test]
    fn a_user_who_declines_at_the_consent_screen_gets_a_clean_answer_not_a_hang() {
        let redirect = parse_redirect(&format!("/?error=access_denied&state={STATE}"), STATE);
        assert_eq!(
            redirect,
            Redirect::Denied {
                reason: "access_denied".to_string()
            }
        );
    }

    #[test]
    fn an_error_wins_over_a_code_when_google_sends_both() {
        let redirect = parse_redirect(
            &format!("/?code=abc&error=access_denied&state={STATE}"),
            STATE,
        );
        assert!(matches!(redirect, Redirect::Denied { .. }));
    }
}
