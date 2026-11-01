//! The only place this crate tells one OS from another.
pub mod call_service;
#[cfg(target_os = "android")]
pub mod kotlin;
pub mod screen;
