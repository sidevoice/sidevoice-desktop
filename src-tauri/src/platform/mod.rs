//! Desktop operations supplied by the host platform.

pub mod update_eligibility;

pub mod computer_name;
pub mod media_controls;
pub mod microphone;
#[cfg(test)]
pub mod pairing_storage;
#[cfg(any(test, feature = "probe"))]
pub mod process_memory;
pub mod reveal;
pub mod tray;
pub mod webview_diagnostics;

pub mod confirmation;
pub mod reset_handoff;
