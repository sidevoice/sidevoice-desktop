//! The only place of this crate that depends on the operating system.

/// Headset buttons and Now Playing: only the computers' bodies exist yet (macOS, Windows, Linux). The phones' (iOS
/// `MPRemoteCommandCenter`, Android `MediaSession`) are sidevoice-desktop#349; until then the module is not in a phone's
/// build, and the mobile app has no media controls (it still tells the page that the app owns the media, so that the
/// page registers none of its own: docs/MOBILE.md).
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub mod media_controls;
pub mod microphone;
pub mod origin;
