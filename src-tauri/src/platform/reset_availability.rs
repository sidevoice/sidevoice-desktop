//! Whether Desktop offers reset (delete all data, uninstall) on this host. When false, callers refuse before any
//! confirmation or data access, and a reset helper only completes the committed wait: the handoff
//! (`reset_handoff`) works on every host either way.
//!
//! Only the reset cleanup itself (data folders, WebKit data, credentials, app removal) differs: its native pieces are
//! supplied for macOS only (`owned_removal`, `webview_data`, `reset_cleanup`). Only this value differs by OS, so there is
//! no per-OS code. It moves into `reset_cleanup` (plan rows 15, 16, 42–44) and disappears once Windows and Linux get
//! their cleanup.

pub const RESET_AVAILABLE: bool = cfg!(target_os = "macos");
