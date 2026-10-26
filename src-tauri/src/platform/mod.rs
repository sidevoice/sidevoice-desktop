//! Desktop operations supplied by the host platform.

pub mod update_eligibility;

pub mod application;
pub mod computer_name;
pub mod media_controls;
pub mod microphone;
#[cfg(test)]
pub mod pairing_storage;
#[cfg(any(test, feature = "probe"))]
pub mod process_memory;
pub mod reveal;
pub mod shortcut_access;
pub mod tray;
pub mod webview_diagnostics;

// The reset caller remains macOS-only until the separate portable-reset work is complete.
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
pub mod confirmation;
