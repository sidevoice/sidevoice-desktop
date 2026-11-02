//! The only place this crate tells one OS from another.
pub mod call_service;
// The Kotlin plugin the Android bodies reach the system through; empty on any other OS.
#[cfg_attr(target_os = "android", path = "platform/kotlin/android.rs")]
#[cfg_attr(not(target_os = "android"), path = "platform/kotlin/other.rs")]
mod kotlin;
pub mod screen;
