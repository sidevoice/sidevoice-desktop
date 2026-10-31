//! The voice call's lifecycle in the app. Its commands are `sidevoice-app-tauri`'s (a plugin over sidevoice-app-core's
//! application host, `AppHost`), which runs the call; what is left here is what only the app knows: when a page has
//! gone, and when the app quits.
//!
//! The call's events reach the room page over the channel it opens when it loads (the plugin's `voice_events`), each
//! sidevoice-voice's `VoiceEvent` as JSON (a take's samples in base64, `voice::take` in sidevoice-app-core), and each step
//! of something said as `{type: "say", data: {key, event}}`, and the audio devices when they change as
//! `{type: "devices", data}`. A page that loads anew finds the call stopped, its models still loaded, and is the only
//! one that hears them.

#[cfg(test)]
mod memory;

use std::time::Duration;

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

/// A reloaded page owns none of its previous lanes, and hears nothing of them: the host numbers the next page.
pub fn page_changed(app: &AppHandle) {
    if let Some(host) = app.try_state::<AppHost>() {
        host.page_changed();
    }
}
