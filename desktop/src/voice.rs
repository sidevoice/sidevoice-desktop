//! The voice call's lifecycle in the app. Its commands are `sidevoice-app-tauri`'s (a plugin over sidevoice-app-core's
//! application host, `AppHost`), which runs the call; what is left here is what only the app knows: when a page has
//! gone, when the app quits, and where the call's events go.
//!
//! The call's events reach the room page as `window.__sidevoiceDesktop.voiceEvent(event)` (`to_page`), each
//! sidevoice-voice's `VoiceEvent` as JSON (a take's samples in base64, `voice::take` in sidevoice-app-core), and each
//! step of something said as `{type: "say", data: {key, event}}`, and the audio devices when they change as
//! `{type: "devices", data}`. A page that loads anew finds the call stopped, its models still loaded.

#[cfg(test)]
mod memory;

use std::time::Duration;

use serde_json::Value;
use sidevoice_app_core::app::AppHost;
use tauri::{AppHandle, Manager};

/// How long quitting waits for the calls to stop and the models to be released.
const RELEASE_LIMIT: Duration = Duration::from_secs(5);

/// Quitting or restarting (`RunEvent::Exit`), before `exit()` runs the static destructors of the C++ libraries the
/// models are made of: the host stops every call and lets every model go (`AppHost::close`), bounded by
/// [`RELEASE_LIMIT`].
pub fn shutdown(app: &AppHandle) {
    let Some(host) = app.try_state::<AppHost>() else { return };
    tauri::async_runtime::block_on(host.close(RELEASE_LIMIT));
}

/// A reloaded page owns none of its previous lanes.
pub fn page_changed(app: &AppHandle) {
    if let Some(host) = app.try_state::<AppHost>() {
        host.page_changed();
    }
}

/// Hands `event` to the room page: `window.__sidevoiceDesktop.voiceEvent(event)`.
pub fn to_page(app: &AppHandle, event: &Value) {
    if let Some(window) = crate::room_window(app) {
        let _ = window.eval(format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.voiceEvent({event})"));
    }
}
