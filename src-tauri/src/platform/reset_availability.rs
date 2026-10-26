//! Whether Desktop offers reset (delete all data, uninstall) on this host. When false, callers refuse before any
//! confirmation or data access, and a reset helper only completes the committed wait: the handoff
//! (`reset_handoff`) works on every host either way.
//!
//! Only the reset cleanup itself (data folders, WebKit data, credentials, app removal) differs: it exists for macOS
//! only and still lives in `data.rs`. Only this value differs by OS, so there is no per-OS code. It moves to the reset
//! cleanup concern when that exists (plan rows 15, 16, 42–44) and disappears once Windows and Linux get their cleanup.

pub const RESET_AVAILABLE: bool = cfg!(target_os = "macos");
