//! OAuth2 for installed applications: PKCE, a loopback redirect, and tokens kept in the
//! Secret Service rather than on disk.

mod flow;
pub mod keyring;
mod loopback;
mod pkce;

pub use flow::{
    Loopback, RefreshRejected, TOKEN_ENDPOINT, authorization_url, begin, exchange_code, refresh,
};
pub use loopback::{Redirect, parse_redirect};
pub use pkce::{Pkce, random_token};
