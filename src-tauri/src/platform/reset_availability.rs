//! Whether Desktop offers reset (delete all data, uninstall) on this host. When false, callers refuse before any
//! confirmation or data access, and a reset helper only completes the committed wait: the handoff
//! (`reset_handoff`) works on every host either way.
//!
//! The deletion itself (`app_data`, with `webview_data` for browser storage) has a body on every host; what is still
//! macOS-only is the rest of the transaction in `data.rs` (connector service, keychain; plan rows 15, 16, 42–44).
//! Only this value differs by OS, so there is no per-OS code. It disappears once Windows and Linux get those steps.

pub const RESET_AVAILABLE: bool = cfg!(target_os = "macos");
