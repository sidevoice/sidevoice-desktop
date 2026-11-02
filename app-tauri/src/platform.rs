//! The only place of this crate that depends on the operating system.

/// The plugin's Kotlin class, which the Android bodies of the concerns below reach.
#[cfg(target_os = "android")]
mod kotlin;
pub mod media_controls;
pub mod microphone;
pub mod origin;
