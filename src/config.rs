//! Where agenda keeps things, and the one file the user has to write by hand.
//!
//! Path resolution is a pure function of the environment rather than a reader of it, so the
//! XDG rules can be tested without mutating process-global state — which in edition 2024 is
//! `unsafe` and, in a test binary running threads, wrong.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The directories agenda reads and writes, resolved once at startup.
pub struct Paths {
    data_dir: PathBuf,
    config_dir: PathBuf,
}

impl Paths {
    /// Resolve from explicit inputs. `Paths::from_env` is the thin wrapper over the real
    /// environment; this is the testable half.
    pub fn new(home: &Path, xdg_data_home: Option<&str>, xdg_config_home: Option<&str>) -> Self {
        Self {
            data_dir: xdg_base(xdg_data_home, home, ".local/share").join("agenda"),
            config_dir: xdg_base(xdg_config_home, home, ".config").join("agenda"),
        }
    }

    pub fn from_env() -> Result<Self> {
        let home = env::var_os("HOME")
            .context("HOME is unset, so there is nowhere to look for agenda's files")?;
        let data = env::var("XDG_DATA_HOME").ok();
        let config = env::var("XDG_CONFIG_HOME").ok();
        Ok(Self::new(
            Path::new(&home),
            data.as_deref(),
            config.as_deref(),
        ))
    }

    /// The SQLite store — the only source of truth the UI reads.
    pub fn database(&self) -> PathBuf {
        self.data_dir.join("agenda.db")
    }

    /// The OAuth client the user creates themselves; see `docs/google-oauth-setup.md`.
    pub fn oauth(&self) -> PathBuf {
        self.config_dir.join("oauth.toml")
    }
}

/// The Google OAuth *client* credentials, which identify the application rather than the
/// user. Deliberately not `derive(Debug)`: a client secret that reaches a log is a secret
/// that has left the keyring's protection.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

impl Credentials {
    /// Parse the two-key file documented in `docs/google-oauth-setup.md`.
    ///
    /// ponytail: a hand-rolled reader for `key = "value"` rather than the `toml` crate,
    /// which is not in the approved dependency set (SPEC §9). Upgrade to `toml` if this
    /// file ever grows past a flat pair of strings.
    pub fn parse(text: &str) -> Result<Self> {
        let mut client_id = None;
        let mut client_secret = None;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                bail!("expected `key = \"value\"` lines, found: {line}");
            };
            match key.trim() {
                "client_id" => client_id = Some(unquote(value.trim())?),
                "client_secret" => client_secret = Some(unquote(value.trim())?),
                // Unknown keys are left alone: the file is the user's, and a future setting
                // they have added by hand is not a reason to refuse to start.
                _ => {}
            }
        }

        let client_id = client_id.context("client_id is missing")?;
        let client_secret = client_secret.context("client_secret is missing")?;
        if client_id.is_empty() || client_secret.is_empty() {
            bail!("client_id and client_secret must both be non-empty");
        }
        Ok(Self {
            client_id,
            client_secret,
        })
    }

    /// `Ok(None)` means the user has not written the file yet — the expected first-run
    /// state, and onboarding rather than an error. `Err` means it exists but is unusable.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .with_context(|| format!("{} is not a usable OAuth client file", path.display()))
                .map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
        }
    }
}

/// The XDG base directory spec requires a value that is not an absolute path to be treated
/// as unset — honouring a relative one would put the database somewhere unpredictable.
fn xdg_base(value: Option<&str>, home: &Path, fallback: &str) -> PathBuf {
    match value {
        Some(value) if Path::new(value).is_absolute() => PathBuf::from(value),
        _ => home.join(fallback),
    }
}

/// Take the contents of a quoted value, ignoring anything after the closing quote so a
/// trailing comment does not end up inside a client secret.
fn unquote(raw: &str) -> Result<String> {
    let quote = match raw.chars().next() {
        Some(quote @ ('"' | '\'')) => quote,
        _ => bail!("value must be quoted, found: {raw}"),
    };
    let rest = &raw[quote.len_utf8()..];
    let end = rest
        .find(quote)
        .with_context(|| format!("unterminated quote in: {raw}"))?;
    Ok(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/tester")
    }

    #[test]
    fn data_directory_falls_back_to_dot_local_share() {
        let paths = Paths::new(&home(), None, None);
        assert_eq!(
            paths.database(),
            PathBuf::from("/home/tester/.local/share/agenda/agenda.db")
        );
    }

    #[test]
    fn config_directory_falls_back_to_dot_config() {
        let paths = Paths::new(&home(), None, None);
        assert_eq!(
            paths.oauth(),
            PathBuf::from("/home/tester/.config/agenda/oauth.toml")
        );
    }

    #[test]
    fn xdg_data_home_overrides_the_default_data_directory() {
        let paths = Paths::new(&home(), Some("/run/data"), None);
        assert_eq!(
            paths.database(),
            PathBuf::from("/run/data/agenda/agenda.db")
        );
    }

    #[test]
    fn xdg_config_home_overrides_the_default_config_directory() {
        let paths = Paths::new(&home(), None, Some("/run/config"));
        assert_eq!(
            paths.oauth(),
            PathBuf::from("/run/config/agenda/oauth.toml")
        );
    }

    #[test]
    fn a_relative_xdg_value_is_ignored_as_the_spec_requires() {
        // The XDG base directory spec says a value that is not an absolute path must be
        // treated as unset. Honouring it would put the database somewhere unpredictable.
        let paths = Paths::new(&home(), Some("relative/path"), Some(""));
        assert_eq!(
            paths.database(),
            PathBuf::from("/home/tester/.local/share/agenda/agenda.db")
        );
        assert_eq!(
            paths.oauth(),
            PathBuf::from("/home/tester/.config/agenda/oauth.toml")
        );
    }

    #[test]
    fn credentials_parse_from_the_documented_two_key_file() {
        let parsed = Credentials::parse(
            r#"client_id = "123.apps.googleusercontent.com"
client_secret = "GOCSPX-abc""#,
        )
        .expect("the documented shape must parse");

        assert_eq!(parsed.client_id, "123.apps.googleusercontent.com");
        assert_eq!(parsed.client_secret, "GOCSPX-abc");
    }

    #[test]
    fn credentials_tolerate_comments_blank_lines_and_single_quotes() {
        let parsed = Credentials::parse(
            r#"# written by hand, per docs/google-oauth-setup.md

client_id = '123.apps.googleusercontent.com'

client_secret = "GOCSPX-abc"   # trailing note
"#,
        )
        .expect("a hand-written file will have comments and stray blank lines");

        assert_eq!(parsed.client_id, "123.apps.googleusercontent.com");
        assert_eq!(parsed.client_secret, "GOCSPX-abc");
    }

    #[test]
    fn credentials_missing_a_key_are_an_error_naming_the_key() {
        let err = Credentials::parse(r#"client_id = "123.apps.googleusercontent.com""#)
            .expect_err("half a credential is not a credential");
        assert!(
            err.to_string().contains("client_secret"),
            "the error must say which key is missing, got: {err}"
        );
    }

    #[test]
    fn credentials_reject_an_empty_value_rather_than_authenticating_with_it() {
        Credentials::parse(
            r#"client_id = ""
client_secret = "GOCSPX-abc""#,
        )
        .expect_err("an empty client_id would fail confusingly at the consent screen");
    }

    #[test]
    fn a_file_that_is_not_key_values_is_an_error_not_a_panic() {
        Credentials::parse("<html>404 Not Found</html>")
            .expect_err("garbage must surface as an error, never a panic");
    }

    #[test]
    fn an_absent_file_is_the_first_run_state_not_an_error() {
        let absent = Credentials::load(Path::new("/nonexistent/agenda/oauth.toml"))
            .expect("a missing file is the expected first-run state");
        assert!(absent.is_none());
    }
}
