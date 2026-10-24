//! Desktop operations supplied by the host platform.

pub mod microphone;

// The reset caller remains macOS-only until the separate portable-reset work is complete.
#[cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
pub mod confirmation;
