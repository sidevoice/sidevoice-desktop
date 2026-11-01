//! Desktop operations supplied by the host platform.

pub mod application;
pub mod diagnostics;
pub mod host;
pub mod locale;
#[cfg(test)]
pub mod pairing_storage;
pub mod reveal;
pub mod shortcut_access;
pub mod tray;

pub mod account;
pub mod app_data;
pub mod card_window;
pub mod confirmation;
pub mod connector_service;
pub mod desktop_geometry;
pub mod pointer_events;
pub mod reset_handoff;
pub mod uninstall;
pub mod webview_data;
pub mod window_visibility;
