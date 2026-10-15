//! The voice call, run by the app itself (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"):
//! sidevoice-voice's `VoiceCall` with the models the app picks on its engine (`models`), the device's own
//! microphone and speaker, the system's default ones or those the page chooses (`NativeDevices`), and WebRTC's echo
//! cancellation between them (`NativeIo`). The page keeps the room, which the call knows nothing of: it tells the room
//! the call's turns, and has the call say what the room sends (`voice_say`), each through a handle in the page that
//! tells how it went.
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

mod models;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use models::{EngineModels, ModelCache};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sidevoice_desktop_core::voice::{
    self as choice, choose, BuildCandidate, Candidate, CatalogCandidates, VoiceSettings,
};
use sidevoice_desktop_engine::{CatalogView, NativeEngines};
use sidevoice_voice::{
    AudioDevices, AudioIo, Events, InputPort, Listening, NativeDevices, SayCancel, SayEvent, SayOptions, SayOutcome,
    SharedNativeAudio, StartOptions, StopReason, Take, VoiceCall, VoiceConfig, VoiceEvent, VoiceModels,
};
use tauri::{AppHandle, Manager, State, Webview};
use tokio::sync::oneshot;

use crate::engine_ipc::EngineState;

/// One native host owns the devices and the loaded model instances. Every call has its own
/// audio lane, VAD stream, recognition state, playback, and event stream.
pub struct VoiceState {
    calls: tokio::sync::Mutex<HashMap<String, Arc<CallLane>>>,
    audio: SharedNativeAudio,
    model_cache: Arc<ModelCache>,
    route: tokio::sync::Mutex<Option<String>>,
    devices: NativeDevices,
    watching: AtomicBool,
}

struct CallLane {
    call: tokio::sync::Mutex<Option<(VoiceCall, Arc<EngineModels>)>>,
    io: Mutex<Option<Box<dyn AudioIo>>>,
    input: InputPort,
    settings: Arc<AtomicU64>,
    lifecycle: Arc<Mutex<Lifecycle>>,
    sayings: Arc<Mutex<HashMap<String, SayCancel>>>,
}

impl Default for VoiceState {
    fn default() -> Self {
        let devices = NativeDevices::default();
        Self {
            calls: tokio::sync::Mutex::new(HashMap::new()),
            audio: SharedNativeAudio::new(devices.clone()),
            model_cache: Arc::new(ModelCache::default()),
            route: tokio::sync::Mutex::new(None),
            devices,
            watching: AtomicBool::new(false),
        }
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

async fn lane(state: &VoiceState, key: &str) -> Result<Arc<CallLane>, Refusal> {
    state.calls.lock().await.get(key).cloned().ok_or_else(|| refusal("bad_request", "Unknown voice call."))
}

/// Attaches a lane now, so routing may be chosen before its settings or start.
#[tauri::command]
pub async fn voice_create_call(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    if !valid_key(&key) {
        return Err(refusal("bad_request", "Invalid voice call key."));
    }
    let mut calls = state.calls.lock().await;
    if calls.contains_key(&key) {
        return Err(refusal("bad_request", "Voice call key already exists."));
    }
    let (io, input) = state.audio.attach();
    calls.insert(
        key,
        Arc::new(CallLane {
            call: tokio::sync::Mutex::new(None),
            io: Mutex::new(Some(io)),
            input,
            settings: Arc::new(AtomicU64::new(0)),
            lifecycle: Arc::new(Mutex::new(Lifecycle::default())),
            sayings: Arc::new(Mutex::new(HashMap::new())),
        }),
    );
    Ok(())
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
    let mut route = state.route.lock().await;
    let target = match key.as_deref() {
        Some(key) => Some(lane(&state, key).await?),
        None => None,
    };
    state.audio.route_input(target.as_ref().map(|lane| &lane.input)).await.map_err(|code| {
        let mut refused = refusal("voice_failed", format!("The microphone route failed: {code}."));
        refused.params.insert("code".into(), json!(code));
        refused
    })?;
    *route = key;
    Ok(())
}

#[tauri::command]
pub async fn voice_dispose_call(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let mut route = state.route.lock().await;
    if route.as_deref() == Some(&key) {
        state.audio.route_input(None).await.map_err(|code| refusal("voice_failed", code))?;
        *route = None;
    }
    let removed = state.calls.lock().await.remove(&key);
    if let Some(lane) = removed {
        stop(&lane).await;
    }
    Ok(())
}

/// How often the devices are listed to tell the page when they change: the system tells cpal of no change.
const DEVICES_EVERY: Duration = Duration::from_secs(2);

/// Whether the call listens, and the `start`s waiting to hear that it does, or why it does not.
#[derive(Default)]
struct Lifecycle {
    listening: bool,
    waiting: Vec<oneshot::Sender<Result<(), String>>>,
}

impl Lifecycle {
    fn settle(&mut self, result: &Result<(), String>) {
        for waiting in self.waiting.drain(..) {
            let _ = waiting.send(result.clone());
        }
    }
}

/// A refusal, in the shape of the bridge's (docs/BRIDGE.md → "Refusals"): a stable key, its parameters, the English
/// sentence for a page that does not know the key. Each carries `code` too, the seam's (sidevoice-voice
/// `js/voice-host.d.ts`): the call's own code when it failed, else the key's, without `voice_` and with dashes.
#[derive(Debug, Serialize)]
pub struct Refusal {
    key: String,
    #[serde(flatten)]
    params: serde_json::Map<String, Value>,
    message: String,
}

fn refusal(key: &str, message: impl Into<String>) -> Refusal {
    let mut params = serde_json::Map::new();
    params.insert("code".into(), json!(key.trim_start_matches("voice_").replace('_', "-")));
    Refusal { key: key.into(), params, message: message.into() }
}

/// A configuration the app cannot make, as the bridge refuses it: the code the page translates (`model-unknown`,
/// `model-unfit`, `catalog-not-found`, `build-not-found`, `vad-unavailable`, `end-of-turn-unavailable`, or the
/// engine's own, such as `credential-missing` or why a chosen build does not run), the model, and what a provider said
/// (`detail`).
fn refused(r: choice::Refusal) -> Refusal {
    let mut refused = refusal(&format!("voice_{}", r.code.replace('-', "_")), r.message);
    refused.params.insert("code".into(), json!(r.code));
    refused.params.insert("model".into(), json!(r.model));
    if let Some(detail) = r.detail {
        refused.params.insert("detail".into(), json!(detail));
    }
    refused
}

fn page(app: &AppHandle, webview: &Webview) -> Result<(), Refusal> {
    crate::room_page(app, webview).map_err(|e| refusal("bad_request", e))
}

fn engines(app: &AppHandle) -> Result<Arc<NativeEngines>, Refusal> {
    let state =
        app.try_state::<EngineState>().ok_or_else(|| refusal("engine_unavailable", "The engine did not start."))?;
    Ok(Arc::clone(&state.engines))
}

/// Sets the person's choices: each slot is a model of one of the engine's catalogues (`{catalog, model}`, checked against
/// what it lists); the app adds the detector and the end-of-turn model, and makes the configuration (every number the
/// person does not choose). The first settings create the call. Later ones never stop it: a changed model is loaded
/// while the one in use goes on (`EngineModels::switch`), takes over once ready, and only then does the configuration
/// that goes with it apply; it resolves then. The latest settings win: ones a later call overtook resolve without
/// changing anything. A model that cannot be loaded leaves the call as it was and rejects `voice_failed` with the
/// engine's code.
#[tauri::command]
pub async fn voice_set_settings(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    settings: VoiceSettings,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let lane = lane(&state, &key).await?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    // Numbered on arrival, before anything is awaited: a later one wins however long this one takes.
    let number = lane.settings.fetch_add(1, Ordering::SeqCst) + 1;
    let counter = Arc::clone(&lane.settings);
    let latest = move || counter.load(Ordering::SeqCst) == number;
    let engines = engines(&app)?;
    let catalogs = {
        let engines = Arc::clone(&engines);
        tauri::async_runtime::spawn_blocking(move || engines.catalogs())
            .await
            .map_err(|e| refusal("internal", e.to_string()))?
            .map_err(|e| refusal(e.key, e.message))?
    };
    let candidates: Vec<CatalogCandidates> = catalogs.iter().map(candidates).collect();
    let choice = choose(&settings, &candidates).map_err(refused)?;
    let config: VoiceConfig =
        serde_json::from_value(choice.config.clone()).map_err(|e| refusal("internal", e.to_string()))?;
    crate::debug(&format!("voice settings: {}", serde_json::to_string(&choice).unwrap_or_default()));
    let mut call = lane.call.lock().await;
    let existing = call.as_ref().map(|(_, models)| Arc::clone(models));
    match existing {
        Some(models) => {
            drop(call);
            if !latest() {
                // A later setSettings overtook this one: it applies instead, models and configuration.
                return Ok(());
            }
            // The call goes on: a slot whose model changes moves to the new one once it is loaded (the old one serving
            // until then), and only then does the configuration that goes with it apply. The call's lock is not held
            // while a model loads, so the call is said to, muted and stopped meanwhile.
            let switched = models.switch(choice, &latest).await.map_err(|code| {
                let mut refused = refusal("voice_failed", format!("The new model could not be loaded: {code}."));
                refused.params.insert("code".into(), json!(code));
                refused
            })?;
            // A later setSettings overtook this one: it applies instead, models and configuration.
            let Some(_switched) = switched else { return Ok(()) };
            if let Some((call, _)) = lane.call.lock().await.as_ref() {
                call.set_config(config);
            }
        }
        None => {
            // The first settings make the call even when later ones came meanwhile (they move it to theirs as they
            // land), so that a start right after these finds a call.
            let models = Arc::new(EngineModels::new(engines.engine(), Arc::clone(&state.model_cache), choice));
            let io = lane
                .io
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| refusal("internal", "Voice lane audio was already taken."))?;
            let (created, events) = VoiceCall::new(Arc::clone(&models) as Arc<dyn VoiceModels>, io, config);
            forward(app.clone(), key, events, Arc::clone(&lane.lifecycle));
            *call = Some((created, models));
        }
    }
    Ok(())
}

/// Loads the models (installing them if they are not), opens the microphone and the speaker and listens. Resolves
/// once the call listens (at once if it does already; with the first when one is pending); rejects with the call's
/// error code, or `stopped` when `stop` comes first. With `take`, the call keeps what the microphone gives from then
/// until that listening ends, and hands it to the page then (`{type: "take", data}`); listening already, it changes
/// nothing.
#[tauri::command]
pub async fn voice_start(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    take: Option<bool>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let lane = lane(&state, &key).await?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    let (sender, started) = oneshot::channel();
    {
        let call = lane.call.lock().await;
        let (call, _) =
            call.as_ref().ok_or_else(|| refusal("voice_settings_missing", "Set the voice settings first."))?;
        let mut lifecycle = lane.lifecycle.lock().unwrap();
        if lifecycle.listening {
            return Ok(());
        }
        lifecycle.waiting.push(sender);
        call.start(StartOptions { take: take.unwrap_or(false) });
    }
    match started.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(code)) => {
            let mut refused = refusal("voice_failed", format!("The call could not start: {code}."));
            refused.params.insert("code".into(), json!(code));
            Err(refused)
        }
        Err(_) => Err(refusal("internal", "the call ended")),
    }
}

/// Stops listening and speaking; the models stay loaded. A `start` still waiting rejects with `stopped`.
#[tauri::command]
pub async fn voice_stop(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let lane = lane(&state, &key).await?;
    stop(&lane).await;
    Ok(())
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
    if !valid_key(&say_key) {
        return Err(refusal("bad_request", "Invalid say key."));
    }
    let lane = lane(&state, &key).await?;
    let call = lane.call.lock().await;
    let Some((call, _)) = call.as_ref() else {
        let stopped = SayEvent::Done { outcome: SayOutcome::NotPlayed { reason: StopReason::Stopped } };
        to_page(&app, &json!({"key": key, "type": "say", "data": {"key": say_key, "event": stopped}}));
        return Ok(());
    };
    let mut saying = call.say(text, SayOptions { language });
    lane.sayings.lock().unwrap().insert(say_key.clone(), saying.canceller());
    let sayings = Arc::clone(&lane.sayings);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = saying.next().await {
            if matches!(event, SayEvent::Done { .. }) {
                sayings.lock().unwrap().remove(&say_key);
            }
            to_page(&app, &json!({"key": key, "type": "say", "data": {"key": say_key, "event": event}}));
        }
    });
    Ok(())
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
    let lane = lane(&state, &key).await?;
    if let Some(cancel) = lane.sayings.lock().unwrap().get(&say_key) {
        cancel.cancel();
    }
    Ok(())
}

/// Mutes or unmutes the microphone; muting ends the open turn with what was said.
#[tauri::command]
pub async fn voice_mute(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    muted: bool,
    key: String,
) -> Result<(), Refusal> {
    with_call(&app, &webview, &state, &key, |call| call.mute(muted)).await
}

/// Cancels what the person said that is not reported yet.
#[tauri::command]
pub async fn voice_cancel_input(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
) -> Result<(), Refusal> {
    with_call(&app, &webview, &state, &key, VoiceCall::cancel_input).await
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

/// A reloaded page owns none of its previous lanes.
pub fn page_changed(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<VoiceState>() {
            let mut route = state.route.lock().await;
            let _ = state.audio.route_input(None).await;
            *route = None;
            let old = std::mem::take(&mut *state.calls.lock().await);
            for (_, lane) in old {
                stop(&lane).await;
            }
        }
    });
}

async fn stop(lane: &CallLane) {
    if let Some((call, _)) = lane.call.lock().await.as_ref() {
        lane.lifecycle.lock().unwrap().settle(&Err("stopped".into()));
        call.stop();
    }
}

async fn with_call(
    app: &AppHandle,
    webview: &Webview,
    state: &VoiceState,
    key: &str,
    action: impl FnOnce(&VoiceCall),
) -> Result<(), Refusal> {
    page(app, webview)?;
    let lane = lane(state, key).await?;
    let call = lane.call.lock().await;
    let (call, _) = call.as_ref().ok_or_else(|| refusal("voice_settings_missing", "Set the voice settings first."))?;
    action(call);
    Ok(())
}

/// Forward events with their immutable call key. Asynchronous delivery never consults the current input route.
fn forward(app: AppHandle, key: String, mut events: Events, lifecycle: Arc<Mutex<Lifecycle>>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
            if let VoiceEvent::Take(take) = &event {
                crate::debug(&format!("voice take: {} samples, {} turns", take.samples.len(), take.turns.len()));
                let mut value = page_take(take);
                value["key"] = json!(key);
                to_page(&app, &value);
                continue;
            }
            match &event {
                VoiceEvent::State(state) => {
                    let mut lifecycle = lifecycle.lock().unwrap();
                    lifecycle.listening = state.listening != Listening::Idle;
                    if lifecycle.listening {
                        lifecycle.settle(&Ok(()));
                    }
                }
                VoiceEvent::Error(error) => lifecycle.lock().unwrap().settle(&Err(error.code.clone())),
                _ => {}
            }
            if !matches!(event, VoiceEvent::Level(_) | VoiceEvent::PlaybackLevel(_)) {
                crate::debug(&format!("voice event {}", serde_json::to_string(&event).unwrap_or_default()));
            }
            if let Ok(mut value) = serde_json::to_value(&event) {
                value["key"] = json!(key);
                to_page(&app, &value);
            }
        }
    });
}

/// A take as the page gets it (core `voice::take`).
fn page_take(take: &Take) -> Value {
    choice::take::page_event(
        take.sample_rate,
        &take.samples,
        take.started_at,
        take.ended_at,
        serde_json::to_value(&take.turns).unwrap_or_default(),
    )
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
