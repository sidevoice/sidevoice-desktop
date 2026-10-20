//! The voice call, run by the app itself (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"):
//! sidevoice-voice's `VoiceCall` with the models the app picks on its engine (`models`), the device's own
//! microphone and speaker, the system's default ones or those the page chooses (`NativeDevices`), and WebRTC's echo
//! cancellation between them (`NativeIo`). A call has an input half, an output half or both, and loads and opens only
//! what its halves use; the page changes it while it runs (`voice_update`) and loads models ahead of it
//! (`voice_preload`). The page keeps the room, which the call knows nothing of: it tells the room the call's turns, and
//! has the call say what the room sends (`voice_say`), each through a handle in the page that tells how it went.
//!
//! The calls themselves (the lanes of the shared devices, their lifecycle, what a stopped call or a preload keeps
//! loaded) are `host`'s, and run without the app; this file is the commands that reach them from the page, and the
//! engine they take their models from.
//!
//! Only the room window's own page may call these (capabilities/room.json, and `room_page`). The call's events reach
//! that page as `window.__sidevoiceDesktop.voiceEvent(event)`, each sidevoice-voice's `VoiceEvent` as JSON (a take's
//! samples in base64, core `voice::take`), and each step of something said as `{type: "say", data: {key, event}}`, and
//! the audio devices when they change as `{type: "devices", data}`. A page that loads anew finds the call stopped, its
//! models still loaded.
//!
//! macOS only: the echo canceller's C++ does not build on Windows yet, and the beta ships on macOS. Elsewhere the page
//! runs the call itself.
#![cfg(target_os = "macos")]

mod host;
mod models;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use sidevoice_desktop_core::voice::{
    choose, BuildCandidate, Candidate, CatalogCandidates, SettingsChange, VoiceChoice, VoiceSettings,
};
use sidevoice_desktop_engine::{CatalogView, NativeEngines};
use sidevoice_voice::AudioDevices;
use tauri::{AppHandle, Manager, State, Webview};

use crate::engine_ipc::EngineState;
use host::{failed, refusal, refused, Applied, Page, Refusal, IDLE_UNLOAD_MINUTES};
use models::{EngineLoader, Halves};

pub use host::VoiceState;

/// How often the devices are listed to tell the page when they change: the system tells cpal of no change.
const DEVICES_EVERY: Duration = Duration::from_secs(2);

fn page(app: &AppHandle, webview: &Webview) -> Result<(), Refusal> {
    crate::room_page(app, webview).map_err(|e| refusal("bad_request", e))
}

/// The room page, as the call's events reach it.
fn page_sink(app: &AppHandle) -> Page {
    let app = app.clone();
    Arc::new(move |event| to_page(&app, &event))
}

fn engines(app: &AppHandle) -> Result<Arc<NativeEngines>, Refusal> {
    let state =
        app.try_state::<EngineState>().ok_or_else(|| refusal("engine_unavailable", "The engine did not start."))?;
    Ok(Arc::clone(&state.engines))
}

/// What the app makes of `settings` on the engine's catalogues as they list their models now: the model of each slot
/// the halves use and the configuration, or the refusal that says why not.
async fn choice_of(engines: &Arc<NativeEngines>, settings: &VoiceSettings) -> Result<VoiceChoice, Refusal> {
    let catalogs = {
        let engines = Arc::clone(engines);
        tauri::async_runtime::spawn_blocking(move || engines.catalogs())
            .await
            .map_err(|e| refusal("internal", e.to_string()))?
            .map_err(|e| refusal(e.key, e.message))?
    };
    let candidates: Vec<CatalogCandidates> = catalogs.iter().map(candidates).collect();
    let choice = choose(settings, &candidates).map_err(refused)?;
    crate::debug(&format!("voice settings: {}", serde_json::to_string(&choice).unwrap_or_default()));
    Ok(choice)
}

fn reset_busy() -> Refusal {
    refusal("reset.busy", "Data reset is under way.")
}

/// Attaches a lane now, so routing may be chosen before its start.
#[tauri::command]
pub async fn voice_create_call(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.create_call(key).await
}

/// The acknowledged next-frame microphone route; None sends silence to all running lanes.
#[tauri::command]
pub async fn voice_route_input(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: Option<String>,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.route_input(key).await
}

/// Stops the call, hands the page what it still had to tell, and releases its lane. The models it loaded stay for its
/// idle minutes.
#[tauri::command]
pub async fn voice_dispose_call(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.dispose_call(&key).await
}

/// Starts the lane's call with the person's choices (`VoiceSettings`, the seam's: an `input` half, an `output` half or
/// both): each slot is a model of one of the engine's catalogues (`{catalog, model}`, checked against what it lists);
/// the app adds the detector (for an input half) and the end-of-turn model (for `smart`), and makes the configuration
/// (every number the person does not choose). Answers once the call is made and asked to start; it loads the models
/// its halves use (installing them if they are not), opens the speaker and, with an input half, the microphone, and
/// `voice_ready` says when it works. With `take`, the call keeps what the microphone gives from then until that
/// listening ends, and hands it to the page then (`{type: "take", data}`).
#[tauri::command]
pub async fn voice_start(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    settings: VoiceSettings,
    take: Option<bool>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(reset_busy)?;
    state.lane(&key).await?;
    let engines = engines(&app)?;
    let choice = choice_of(&engines, &settings).await?;
    let loader = EngineLoader::new(engines.engine(), Arc::clone(&state.model_cache));
    let applied = Applied { halves: Halves::new(&loader, &choice, None), settings };
    state.open(&key, applied, take.unwrap_or(false), page_sink(&app)).await
}

/// Resolves once the call works: at its first state that is not idle (`off` for a call without an input half); rejects
/// with the call's error code, or `stopped` when `stop` comes first.
#[tauri::command]
pub async fn voice_ready(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(reset_busy)?;
    state.ready(&key).await
}

/// Changes the call while it runs (`SettingsChange`: a half that is absent stays, `null` removes it, settings replace
/// it or add it), without restarting it: a half whose slots keep their models keeps what it loaded, other models load
/// while the call goes on as it was, and each half's change takes effect at its safe point (the input half's at the
/// next turn, the output half's at the next utterance). Changes are made in the order they come. Resolves once the
/// change is accepted, not applied; refuses what leaves the call no half, or a model the app cannot make, changing
/// nothing. A model that cannot load is an error of the call with its code, and its half stays as it was.
#[tauri::command]
pub async fn voice_update(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    change: SettingsChange,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let lane = state.lane(&key).await?;
    let _turn = lane.changing.lock().await;
    let _permit = crate::data::admit(&app).ok_or_else(reset_busy)?;
    let current = state.applied(&key).await?;
    let settings = current.settings.changed(change);
    let engines = engines(&app)?;
    let choice = choice_of(&engines, &settings).await?;
    let loader = EngineLoader::new(engines.engine(), Arc::clone(&state.model_cache));
    let applied = Applied { halves: Halves::new(&loader, &choice, Some(&current.halves)), settings };
    state.update(&key, applied).await
}

/// Loads the models `settings` use into the host, ahead of a call: the slots a `voice_start` of them would ask for (an
/// absent half loads none of its own, `silence` no end-of-turn model), once for a model already loaded or loading,
/// opening no microphone and no speaker. They are held like a stopped call's: a call that starts on the same models
/// takes them without loading, and they are let go `idle_unload_minutes` (10 by default) after the end of the load.
/// Resolves once every slot is loaded; rejects `voice_failed` with the code of a slot that cannot load, what else
/// loaded staying loaded.
#[tauri::command]
pub async fn voice_preload(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    settings: VoiceSettings,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(reset_busy)?;
    let engines = engines(&app)?;
    let choice = choice_of(&engines, &settings).await?;
    let loader = EngineLoader::new(engines.engine(), Arc::clone(&state.model_cache));
    let minutes = settings.idle_unload_minutes.unwrap_or(IDLE_UNLOAD_MINUTES);
    state
        .preload(Halves::new(&loader, &choice, None), minutes)
        .await
        .map_err(|code| failed("The models could not be loaded", code))
}

/// Stops listening and speaking; the lane stays until `voice_dispose_call`. A `voice_ready` still waiting rejects with
/// `stopped`. Answers once the page has what the stop ends (`true`), or at once with `false` before the call starts.
#[tauri::command]
pub async fn voice_stop(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<bool, Refusal> {
    page(&app, &webview)?;
    state.stop(&key).await
}

/// Says text on one call; its keyed events return only to that call's handle.
#[tauri::command]
pub async fn voice_say(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
    say_key: String,
    text: String,
    language: Option<String>,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.say(&key, say_key, text, language, page_sink(&app)).await
}

/// Cancels what is being said under `key`: the part not yet heard is dropped, and its outcome says `cancelled`. Once
/// it has ended, nothing.
#[tauri::command]
pub async fn voice_cancel_say(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
    say_key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.cancel_say(&key, &say_key).await
}

/// Mutes or unmutes the microphone for this call: it hears silence, and an open turn ends only as silence ends it.
/// Kept from the call's creation, before it starts too.
#[tauri::command]
pub async fn voice_mute(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    muted: bool,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.mute(&key, muted).await
}

/// Cancels what the person said that is not reported yet; nothing before the call starts.
#[tauri::command]
pub async fn voice_cancel_input(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.cancel_input(&key).await
}

/// Closes the open turn now with what was captured so far: it is transcribed and told like any turn. It neither
/// mutes, nor touches the playback or how later turns end. Resolves once the turn is closed, also when there is
/// nothing to close (the call not started, no open turn, the call stopped).
#[tauri::command]
pub async fn voice_end_input(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    state.end_input(&key).await
}

/// The system's microphones and speakers and the ones chosen (`null`: the system's default), as the seam's
/// `VoiceAudioDevices`: `{inputs, outputs, input, output, outputSelectable, lost?}`, each device `{id, label}`. A chosen
/// device that went away is the default one from then, said once as `lost`. From now on the page is told when they
/// change (`{type: "devices", data}`).
#[tauri::command]
pub async fn voice_audio_devices(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
) -> Result<Value, Refusal> {
    page(&app, &webview)?;
    watch_devices(&app, &state);
    let devices = state.devices.clone();
    listed(tauri::async_runtime::spawn_blocking(move || devices.list()).await)
}

/// What `voice_set_audio_devices` chooses: a missing kind stays as it is, `null` is the system's default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioChoice {
    #[serde(default, deserialize_with = "present")]
    input: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    output: Option<Option<String>>,
}

/// A field that is there, `null` included.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

/// Chooses the microphone, the speaker or both, at any time: a call that runs switches its microphone at once and its
/// speaker once what it is playing has ended. Answers the devices as `voice_audio_devices` does; rejects `{key:
/// "voice_audio_device_unknown", code: "audio-device-unknown"}` for an id the system does not list, nothing changed.
#[tauri::command]
pub async fn voice_set_audio_devices(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    choice: AudioChoice,
) -> Result<Value, Refusal> {
    page(&app, &webview)?;
    watch_devices(&app, &state);
    let devices = state.devices.clone();
    listed(tauri::async_runtime::spawn_blocking(move || devices.choose(choice.input, choice.output)).await)
}

/// The devices as the seam has them, or the app's refusal with the code the system's listing gave.
fn listed(listing: Result<Result<AudioDevices, String>, tauri::Error>) -> Result<Value, Refusal> {
    let devices = listing.map_err(|e| refusal("internal", e.to_string()))?.map_err(|code| {
        let mut refused = refusal(&format!("voice_{}", code.replace('-', "_")), format!("The audio devices: {code}."));
        refused.params.insert("code".into(), json!(code));
        refused
    })?;
    Ok(seam_devices(&devices))
}

/// `devices` as the seam's `VoiceAudioDevices`: natively, the speaker can always be chosen.
fn seam_devices(devices: &AudioDevices) -> Value {
    let mut value = json!(devices);
    value["outputSelectable"] = json!(true);
    value
}

/// Lists the devices every `DEVICES_EVERY` from the first call on, and tells the page each change: other devices, another
/// choice, or a chosen one that went away.
fn watch_devices(app: &AppHandle, state: &VoiceState) {
    if state.watching.swap(true, Ordering::AcqRel) {
        return;
    }
    let (app, devices) = (app.clone(), state.devices.clone());
    tauri::async_runtime::spawn(async move {
        let mut last: Option<AudioDevices> = None;
        loop {
            let listing = devices.clone();
            let Ok(Ok(now)) = tauri::async_runtime::spawn_blocking(move || listing.list()).await else {
                tokio::time::sleep(DEVICES_EVERY).await;
                continue;
            };
            let changed = now.lost.is_some()
                || last.as_ref().is_some_and(|last| {
                    (&last.inputs, &last.outputs, &last.input, &last.output)
                        != (&now.inputs, &now.outputs, &now.input, &now.output)
                });
            if changed {
                to_page(&app, &json!({"type": "devices", "data": seam_devices(&now)}));
            }
            last = Some(now);
            tokio::time::sleep(DEVICES_EVERY).await;
        }
    });
}

/// Quitting or restarting (`RunEvent::Exit`), before `exit()` runs the static destructors of the C++ libraries the
/// models are made of: every call is stopped and released, so the devices close, and every model is let go. One left
/// loaded is still alive in those destructors, and whisper.cpp's Metal device aborts in its own when one is (#121).
/// Bounded: a release that does not finish in time is logged, and the quit goes on.
pub fn shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<VoiceState>() else { return };
    let held = tauri::async_runtime::block_on(state.close(RELEASE_LIMIT, Duration::from_millis(50)));
    if held.is_empty() {
        crate::debug("voice: every model released");
    } else {
        log::error!("voice: models still held at exit after {RELEASE_LIMIT:?}: {}", held.join(", "));
        crate::debug(&format!("voice: models still held at exit: {}", held.join(", ")));
    }
}

/// How long quitting waits for the calls to stop and the models to be released.
const RELEASE_LIMIT: Duration = Duration::from_secs(5);

/// A reloaded page owns none of its previous lanes.
pub fn page_changed(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<VoiceState>() {
            state.clear().await;
        }
    });
}

/// Hands `event` to the room page: `window.__sidevoiceDesktop.voiceEvent(event)`.
fn to_page(app: &AppHandle, event: &Value) {
    if let Some(window) = crate::room_window(app) {
        let _ = window.eval(format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.voiceEvent({event})"));
    }
}

/// A catalogue of the engine, as choosing from it needs: a local model runs here when one of its builds does; a
/// provider's always does. A build that does not run says why by its first reason's code.
pub(crate) fn candidates(view: &CatalogView) -> CatalogCandidates {
    CatalogCandidates {
        id: view.id.clone(),
        reason: view.status.reason.as_ref().map(|reason| reason.code.clone()),
        detail: view.status.detail.clone(),
        models: view
            .models
            .iter()
            .map(|model| Candidate {
                id: model.id.clone(),
                family: model.local.as_ref().map(|local| local.family.clone()),
                capabilities: model.capabilities.iter().map(|c| c.to_string()).collect(),
                runs: model.local.as_ref().is_none_or(|local| local.builds.iter().any(|build| build.available)),
                builds: model.local.as_ref().map_or_else(Vec::new, |local| {
                    local
                        .builds
                        .iter()
                        .map(|build| BuildCandidate {
                            id: build.id.clone(),
                            unfit: (!build.available).then(|| {
                                build.reasons.first().map_or("no-build-available".into(), |reason| reason.code.clone())
                            }),
                        })
                        .collect()
                }),
            })
            .collect(),
    }
}
