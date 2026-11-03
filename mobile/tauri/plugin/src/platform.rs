//! The only place this crate tells one OS from another.
pub mod call_service;
pub mod call_surface;
// The Kotlin plugin the Android bodies reach the system through: only Android has one.
#[cfg(target_os = "android")]
#[path = "platform/kotlin/android.rs"]
mod kotlin;
pub mod audio_outputs;
pub mod screen;
