//! OAuth2 for installed applications: PKCE, a loopback redirect, and tokens kept in the
//! Secret Service rather than on disk.

mod loopback;

pub use loopback::{Redirect, parse_redirect};
