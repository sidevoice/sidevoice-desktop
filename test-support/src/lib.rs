//! Reusable test doubles. No production dependency on this crate.
#![allow(clippy::disallowed_macros, reason = "test doubles print for the test that runs them")]
pub mod fake_core;
pub mod fake_registry;
pub mod fixture;
mod http;
pub mod platform;
