//! Desktop operations supplied by the host platform.

pub mod update_eligibility;

pub mod application;
pub mod computer_name;
#[cfg(feature = "probe")]
pub mod exit_diagnostics;
pub mod host_capabilities;
pub mod locale;
pub mod media_controls;
pub mod microphone;
#[cfg(test)]
pub mod pairing_storage;
#[cfg(any(test, feature = "probe"))]
pub mod process_memory;
pub mod reveal;
pub mod shortcut_access;
pub mod tray;
#[cfg(any(test, feature = "probe"))]
pub mod voice_holds;
pub mod webview_diagnostics;

pub mod confirmation;
pub mod reset_availability;
pub mod reset_handoff;

// Windows/Linux reset remains hidden until the portable transaction is assembled.
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
pub mod webview_data;
