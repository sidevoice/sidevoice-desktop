//! Desktop operations supplied by the host platform.

pub mod computer_name;
pub mod microphone;
#[cfg(any(test, feature = "probe"))]
pub mod process_memory;
pub mod reveal;
pub mod tray;
pub mod webview_diagnostics;

// The reset caller remains macOS-only until the separate portable-reset work is complete.
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
pub mod confirmation;

pub mod card_window;
pub mod desktop_geometry;
pub mod window_visibility;
