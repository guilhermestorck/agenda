//! Google Calendar API v3.

pub mod client;
pub mod wire;

pub use client::{Session, SyncTokenGone, UserInfo, userinfo};
