//! The voice call's commands (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"): what the page
//! asks, handed to the application host, which runs the call. Each is the caller check, the arguments and one call. The
//! call's events do not come back through here: the host hands them to the app's `EventSink`.
//!
//! Only the page the app owns the calls for may call these (the capability, and the app's [`crate::init`] caller).

use serde_json::Value;
use sidevoice_app_core::app::{AppHost, AudioChoice, PageGeneration};
use sidevoice_app_core::voice::host::{refusal, Refusal};
use sidevoice_app_core::voice::{SettingsChange, VoiceSettings};
use tauri::ipc::Channel;
use tauri::{Runtime, State, Webview};

use crate::pages::Document;
use crate::{Callers, Pages};

fn allowed<R: Runtime>(callers: &Callers<R>, webview: &Webview<R>) -> Result<(), Refusal> {
    callers.check(webview).map_err(|why| refusal("bad_request", why))
}

/// The page opens the channel its events come on, and learns its number: the generation it sends back with every command
/// below. A page asks once, when it loads, and says which document it is (`document`, its own start time), so that a
/// request of a page that was replaced meanwhile, and reached the app late, is refused `page_stale`. The events of the calls its lanes make are sent over the channel, in order,
/// while it is the current page; a page that is replaced hears nothing more, and the next one opens its own.
#[tauri::command]
pub async fn voice_events<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    pages: State<'_, Pages>,
    document: f64,
    events: Channel<Value>,
) -> Result<PageGeneration, Refusal> {
    allowed(&callers, &webview)?;
    pages.register(Document(document), || host.page(), events)
}

/// Attaches a lane now, so routing may be chosen before its start.
#[tauri::command]
pub async fn voice_create_call<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.create_call(page, key).await
}

/// The acknowledged next-frame microphone route; None sends silence to all running lanes.
#[tauri::command]
pub async fn voice_route_input<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: Option<String>,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.route_input(page, key).await
}

/// Stops the call, hands the page what it still had to tell, and releases its lane. The models it loaded stay for their
/// idle minutes.
#[tauri::command]
pub async fn voice_dispose_call<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.dispose_call(page, &key).await
}

/// Starts the lane's call with the person's choices (`VoiceSettings`, the seam's); with `take`, the call keeps what the
/// microphone gives from then until that listening ends, and hands it to the page then (`{type: "take", data}`).
#[tauri::command]
pub async fn voice_start<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    settings: VoiceSettings,
    take: Option<bool>,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.start(page, &key, settings, take.unwrap_or(false)).await
}

/// Resolves once the call works: at its first state that is not idle (`off` for a call without an input half); rejects
/// with the call's error code, or `stopped` when `voice_stop` comes first.
#[tauri::command]
pub async fn voice_ready<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.ready(page, &key).await
}

/// Changes the call while it runs (`SettingsChange`: a half that is absent stays, `null` removes it, settings replace
/// it or add it), without restarting it. Changes are made in the order they come.
#[tauri::command]
pub async fn voice_update<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    change: SettingsChange,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.update(page, &key, change).await
}

/// Loads the models `settings` use into the host, ahead of a call; they are let go `idle_unload_minutes` (10 by default)
/// after the end of the load.
#[tauri::command]
pub async fn voice_preload<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    settings: VoiceSettings,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.preload(page, settings).await
}

/// Stops listening and speaking; the lane stays until `voice_dispose_call`. A `voice_ready` still waiting rejects with
/// `stopped`. Answers once the page has what the stop ends (`true`), or at once with `false` before the call starts.
#[tauri::command]
pub async fn voice_stop<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
) -> Result<bool, Refusal> {
    allowed(&callers, &webview)?;
    host.stop(page, &key).await
}

/// Says text on one call; its keyed events return only to that call's handle.
#[tauri::command]
// The page's own arguments (five) and the plugin's state (three): a command is its wire.
#[allow(clippy::too_many_arguments)]
pub async fn voice_say<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
    say_key: String,
    text: String,
    language: Option<String>,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.say(page, &key, say_key, text, language).await
}

/// Cancels what is being said under `say_key`: the part not yet heard is dropped, and its outcome says `cancelled`. Once
/// it has ended, nothing.
#[tauri::command]
pub async fn voice_cancel_say<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
    say_key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.cancel_say(page, &key, &say_key).await
}

/// Mutes or unmutes the microphone for this call: it hears silence, and an open turn ends only as silence ends it. Kept
/// from the call's creation, before it starts too.
#[tauri::command]
pub async fn voice_mute<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    muted: bool,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.mute(page, &key, muted).await
}

/// Cancels what the person said that is not reported yet; nothing before the call starts.
#[tauri::command]
pub async fn voice_cancel_input<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.cancel_input(page, &key).await
}

/// Closes the open turn now with what was captured so far: it is transcribed and told like any turn. Resolves once the
/// turn is closed, also when there is nothing to close (the call not started, no open turn, the call stopped).
#[tauri::command]
pub async fn voice_end_input<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    key: String,
) -> Result<(), Refusal> {
    allowed(&callers, &webview)?;
    host.end_input(page, &key).await
}

/// The system's microphones and speakers and the ones chosen (`null`: the system's default), as the seam's
/// `VoiceAudioDevices`. From now on the page is told when they change (`{type: "devices", data}`).
#[tauri::command]
pub async fn voice_audio_devices<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
) -> Result<Value, Refusal> {
    allowed(&callers, &webview)?;
    host.audio_devices(page).await
}

/// Chooses the microphone, the speaker or both, at any time. Answers the devices as `voice_audio_devices` does.
#[tauri::command]
pub async fn voice_set_audio_devices<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    page: PageGeneration,
    choice: AudioChoice,
) -> Result<Value, Refusal> {
    allowed(&callers, &webview)?;
    host.set_audio_devices(page, choice).await
}
