//! The scripts the app injects into its pages (`js/`, docs/BRIDGE.md): the crate that injects them ships them, and
//! `sidevoice-desktop-core` composes them.

/// Removes the WebView's default context menu (js/no-context-menu.js); appended to the scripts of every window.
pub const NO_CONTEXT_MENU: &str = include_str!("../js/no-context-menu.js");

/// The desktop's part of the room's bridge script (what only this app has, and the assembly of `host`), with its
/// placeholders still in place.
pub const DESKTOP_BRIDGE: &str = include_str!("../js/desktop-bridge.js");

/// The call controls card's bridge script, with its placeholders still in place.
pub const CALL_CONTROLS_BRIDGE: &str = include_str!("../js/call-controls-bridge.js");
