//! agenda's non-UI half.
//!
//! Split from the binary so that each module's public surface is the crate's API rather
//! than unreachable-from-`main` code. SPEC §8 forbids `#[allow(dead_code)]` to hide
//! unfinished wiring; a library lets a finished, tested module wait for its consumer
//! without either a suppression or a warning.

pub mod auth;
pub mod config;
pub mod store;
