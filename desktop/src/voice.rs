//! The voice call's commands (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"): what the room
//! page asks, handed to sidevoice-app-core's application host (`AppHost`), which runs the call: sidevoice-voice's
//! `VoiceCall` with the models the app picks on its engine, the device's own microphone and speaker, the system's
//! default ones or those the page chooses, and Sonora's echo cancellation between them. A call has an input half, an
//! output half or both, and loads and opens only what its halves use; the page changes it while it runs
//! (`voice_update`) and loads models ahead of it (`voice_preload`). The page keeps the room, which the call knows
//! nothing of: it tells the room the call's turns, and has the call say what the room sends (`voice_say`), each
//! through a handle in the page that tells how it went.
//!
//! This file is the commands: the caller check, the arguments, and nothing of what they do (that is the host's, and
//! runs without the app).
//!
//! Only the room window's own page may call these (capabilities/room.json, and `room_page`). The call's events reach
//! that page as `window.__sidevoiceDesktop.voiceEvent(event)` (`to_page`), each sidevoice-voice's `VoiceEvent` as JSON
//! (a take's samples in base64, `voice::take` in sidevoice-app-core), and each step of something said as
//! `{type: "say", data: {key, event}}`, and the audio devices when they change as `{type: "devices", data}`. A page
//! that loads anew finds the call stopped, its models still loaded.

#[cfg(test)]
mod memory;

use std::time::Duration;

use serde_json::Value;
use sidevoice_app_core::app::{AppHost, AudioChoice};
use sidevoice_app_core::voice::host::{refusal, Refusal};
use sidevoice_app_core::voice::{SettingsChange, VoiceSettings};
use tauri::{AppHandle, Manager, State, Webview};

fn page(app: &AppHandle, webview: &Webview) -> Result<(), Refusal> {
    crate::room_page(app, webview).map_err(|e| refusal("bad_request", e))
}

/// Attaches a lane now, so routing may be chosen before its start.
#[tauri::command]
pub async fn voice_create_call(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.create_call(key).await
}

/// The acknowledged next-frame microphone route; None sends silence to all running lanes.
#[tauri::command]
pub async fn voice_route_input(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: Option<String>,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.route_input(key).await
}

/// Stops the call, hands the page what it still had to tell, and releases its lane. The models it loaded stay for their
/// idle minutes.
#[tauri::command]
pub async fn voice_dispose_call(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.dispose_call(&key).await
}

/// Starts the lane's call with the person's choices (`VoiceSettings`, the seam's); with `take`, the call keeps what the
/// microphone gives from then until that listening ends, and hands it to the page then (`{type: "take", data}`).
#[tauri::command]
pub async fn voice_start(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    settings: VoiceSettings,
    take: Option<bool>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.start(&key, settings, take.unwrap_or(false)).await
}

/// Resolves once the call works: at its first state that is not idle (`off` for a call without an input half); rejects
/// with the call's error code, or `stopped` when `stop` comes first.
#[tauri::command]
pub async fn voice_ready(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.ready(&key).await
}

/// Changes the call while it runs (`SettingsChange`: a half that is absent stays, `null` removes it, settings replace
/// it or add it), without restarting it. Changes are made in the order they come.
#[tauri::command]
pub async fn voice_update(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    change: SettingsChange,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.update(&key, change).await
}

/// Loads the models `settings` use into the host, ahead of a call; they are let go `idle_unload_minutes` (10 by default)
/// after the end of the load.
#[tauri::command]
pub async fn voice_preload(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    settings: VoiceSettings,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.preload(settings).await
}

/// Stops listening and speaking; the lane stays until `voice_dispose_call`. A `voice_ready` still waiting rejects with
/// `stopped`. Answers once the page has what the stop ends (`true`), or at once with `false` before the call starts.
#[tauri::command]
pub async fn voice_stop(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
) -> Result<bool, Refusal> {
    page(&app, &webview)?;
    host.stop(&key).await
}

/// Says text on one call; its keyed events return only to that call's handle.
#[tauri::command]
pub async fn voice_say(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
    say_key: String,
    text: String,
    language: Option<String>,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.say(&key, say_key, text, language).await
}

/// Cancels what is being said under `key`: the part not yet heard is dropped, and its outcome says `cancelled`. Once it
/// has ended, nothing.
#[tauri::command]
pub async fn voice_cancel_say(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
    say_key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.cancel_say(&key, &say_key).await
}

/// Mutes or unmutes the microphone for this call: it hears silence, and an open turn ends only as silence ends it. Kept
/// from the call's creation, before it starts too.
#[tauri::command]
pub async fn voice_mute(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    muted: bool,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.mute(&key, muted).await
}

/// Cancels what the person said that is not reported yet; nothing before the call starts.
#[tauri::command]
pub async fn voice_cancel_input(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.cancel_input(&key).await
}

/// Closes the open turn now with what was captured so far: it is transcribed and told like any turn. Resolves once the
/// turn is closed, also when there is nothing to close (the call not started, no open turn, the call stopped).
#[tauri::command]
pub async fn voice_end_input(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    host.end_input(&key).await
}

/// The system's microphones and speakers and the ones chosen (`null`: the system's default), as the seam's
/// `VoiceAudioDevices`. From now on the page is told when they change (`{type: "devices", data}`).
#[tauri::command]
pub async fn voice_audio_devices(app: AppHandle, webview: Webview, host: State<'_, AppHost>) -> Result<Value, Refusal> {
    page(&app, &webview)?;
    host.audio_devices().await
}

/// Chooses the microphone, the speaker or both, at any time. Answers the devices as `voice_audio_devices` does.
#[tauri::command]
pub async fn voice_set_audio_devices(
    app: AppHandle,
    webview: Webview,
    host: State<'_, AppHost>,
    choice: AudioChoice,
) -> Result<Value, Refusal> {
    page(&app, &webview)?;
    host.set_audio_devices(choice).await
}

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
