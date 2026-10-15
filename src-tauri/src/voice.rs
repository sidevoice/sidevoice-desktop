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
use models::EngineModels;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sidevoice_desktop_core::voice::{
    self as choice, choose, BuildCandidate, Candidate, CatalogCandidates, VoiceSettings,
};
use sidevoice_desktop_engine::{CatalogView, NativeEngines};
use sidevoice_voice::{
    AudioDevices, Events, Listening, NativeDevices, NativeIo, SayCancel, SayEvent, SayOptions, SayOutcome,
    StartOptions, StopReason, Take, VoiceCall, VoiceConfig, VoiceEvent, VoiceModels,
};
use tauri::{AppHandle, Manager, State, Webview};
use tokio::sync::{oneshot, watch};

use crate::engine_ipc::EngineState;

/// The call, once the page has set its settings, with the models it runs on (whose choice changes in place).
#[derive(Default)]
pub struct VoiceState {
    call: tokio::sync::Mutex<Option<(VoiceCall, Arc<EngineModels>)>>,
    /// The `setSettings` asked for so far, numbered as they arrive: only the latest changes the call.
    settings: Arc<AtomicU64>,
    lifecycle: Arc<Mutex<Lifecycle>>,
    /// What is being said, by the page's key for it, until it is done.
    sayings: Arc<Mutex<HashMap<String, SayCancel>>>,
    /// Native say ids in `agent-interrupted` become the bridge keys returned by `host.voice.say`.
    native_keys: Arc<Mutex<HashMap<String, (String, watch::Sender<bool>)>>>,
    /// Which microphone and speaker the call uses; the call has a clone.
    devices: NativeDevices,
    /// Whether the devices are watched for changes (from the first time the page asks for them).
    watching: AtomicBool,
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
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    // Numbered on arrival, before anything is awaited: a later one wins however long this one takes.
    let number = state.settings.fetch_add(1, Ordering::SeqCst) + 1;
    let counter = Arc::clone(&state.settings);
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
    let mut call = state.call.lock().await;
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
            if let Some((call, _)) = state.call.lock().await.as_ref() {
                call.set_config(config);
            }
        }
        None => {
            // The first settings make the call even when later ones came meanwhile (they move it to theirs as they
            // land), so that a start right after these finds a call.
            let models = Arc::new(EngineModels::new(engines.engine(), choice));
            let io = Box::new(NativeIo::with_devices(state.devices.clone()));
            let (created, events) = VoiceCall::new(Arc::clone(&models) as Arc<dyn VoiceModels>, io, config);
            forward(app.clone(), events, Arc::clone(&state.lifecycle), Arc::clone(&state.native_keys));
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
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let (sender, started) = oneshot::channel();
    {
        let call = state.call.lock().await;
        let (call, _) =
            call.as_ref().ok_or_else(|| refusal("voice_settings_missing", "Set the voice settings first."))?;
        let mut lifecycle = state.lifecycle.lock().unwrap();
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
pub async fn voice_stop(app: AppHandle, webview: Webview, state: State<'_, VoiceState>) -> Result<(), Refusal> {
    page(&app, &webview)?;
    stop(&state).await;
    Ok(())
}

/// Says `text` (in `language`, the settings' when absent) after whatever is being said, under `key`, the page's handle
/// for it: each step reaches the page as `voiceEvent({type: "say", data: {key, event}})`, the last being `done` with
/// its outcome. With no call yet it is not played (`stopped`).
#[tauri::command]
pub async fn voice_say(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    key: String,
    text: String,
    language: Option<String>,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    if key.is_empty() || key.len() > 64 || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(refusal("bad_request", format!("not a say key: {key:?}")));
    }
    let call = state.call.lock().await;
    let Some((call, _)) = call.as_ref() else {
        let stopped = SayEvent::Done { outcome: SayOutcome::NotPlayed { reason: StopReason::Stopped } };
        to_page(&app, &json!({"type": "say", "data": {"key": key, "event": stopped}}));
        return Ok(());
    };
    // Hold the reverse map lock while the call accepts the say: its event loop may emit an interruption immediately.
    let mut native_keys = state.native_keys.lock().unwrap();
    let mut saying = call.say(text, SayOptions { language });
    let native_id = saying.id().to_owned();
    let (cut_sent, mut cut_forwarded) = watch::channel(false);
    native_keys.insert(native_id.clone(), (key.clone(), cut_sent));
    state.sayings.lock().unwrap().insert(key.clone(), saying.canceller());
    drop(native_keys);
    let sayings = Arc::clone(&state.sayings);
    let native_keys = Arc::clone(&state.native_keys);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = saying.next().await {
            let done = matches!(event, SayEvent::Done { .. });
            let interrupted = matches!(event, SayEvent::Done { outcome: SayOutcome::HeardUpTo { reason: StopReason::BargeIn, .. } | SayOutcome::NotPlayed { reason: StopReason::BargeIn } });
            if done {
                sayings.lock().unwrap().remove(&key);
            }
            if interrupted {
                // The call emits the cut before this terminal step, but the two Rust streams have separate tasks.
                // Keep the web's deduplication order even when the terminal receiver is scheduled first.
                while !*cut_forwarded.borrow_and_update() {
                    if cut_forwarded.changed().await.is_err() {
                        break;
                    }
                }
            }
            to_page(&app, &json!({"type": "say", "data": {"key": key, "event": event}}));
            // The call emits `agent-interrupted` separately from the terminal say event. Keep this mapping until
            // that event is forwarded, even if the two receiver tasks are scheduled in the opposite order.
            if done && !interrupted {
                native_keys.lock().unwrap().remove(&native_id);
            }
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
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    if let Some(cancel) = state.sayings.lock().unwrap().get(&key) {
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
) -> Result<(), Refusal> {
    with_call(&app, &webview, &state, |call| call.mute(muted)).await
}

/// Cancels what the person said that is not reported yet.
#[tauri::command]
pub async fn voice_cancel_input(app: AppHandle, webview: Webview, state: State<'_, VoiceState>) -> Result<(), Refusal> {
    with_call(&app, &webview, &state, VoiceCall::cancel_input).await
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

/// The room page loaded anew: its call is stopped, and the next page starts it again.
pub fn page_changed(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<VoiceState>() {
            stop(&state).await;
        }
    });
}

async fn stop(state: &VoiceState) {
    if let Some((call, _)) = state.call.lock().await.as_ref() {
        state.lifecycle.lock().unwrap().settle(&Err("stopped".into()));
        call.stop();
    }
}

async fn with_call(
    app: &AppHandle,
    webview: &Webview,
    state: &VoiceState,
    action: impl FnOnce(&VoiceCall),
) -> Result<(), Refusal> {
    page(app, webview)?;
    let call = state.call.lock().await;
    let (call, _) = call.as_ref().ok_or_else(|| refusal("voice_settings_missing", "Set the voice settings first."))?;
    action(call);
    Ok(())
}

/// Hands every event of the call to the room page, in order, and keeps the lifecycle: the call listens from its first
/// state other than idle, which settles the `start`s waiting, as does the first error while they wait.
fn forward(
    app: AppHandle,
    mut events: Events,
    lifecycle: Arc<Mutex<Lifecycle>>,
    native_keys: Arc<Mutex<HashMap<String, (String, watch::Sender<bool>)>>>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
            // A take goes to the page with its samples in base64, and to the log only as its size.
            if let VoiceEvent::Take(take) = &event {
                crate::debug(&format!("voice take: {} samples, {} turns", take.samples.len(), take.turns.len()));
                to_page(&app, &page_take(take));
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
            if let Some((value, cut_sent)) = event_for_page(&event, &native_keys) {
                to_page(&app, &value);
                if let Some(sent) = cut_sent {
                    sent.send_replace(true);
                }
            }
        }
    });
}

/// Only the native say id changes: the page knows the bridge key returned by its `say` handle.
fn event_for_page(
    event: &VoiceEvent,
    native_keys: &Mutex<HashMap<String, (String, watch::Sender<bool>)>>,
) -> Option<(Value, Option<watch::Sender<bool>>)> {
    let mut value = serde_json::to_value(event).ok()?;
    let mut cut_sent = None;
    if let VoiceEvent::AgentInterrupted(cut) = event {
        let (key, sent) = native_keys.lock().unwrap().remove(&cut.utterance_id)?;
        value["data"]["utterance_id"] = json!(key);
        cut_sent = Some(sent);
    }
    Some((value, cut_sent))
}

#[cfg(test)]
mod turns_tests {
    use super::*;
    use sidevoice_voice::AgentInterrupted;

    #[test]
    fn native_interruption_uses_the_page_handle_and_keeps_scalar_extent_and_zero() {
        let (first, _) = watch::channel(false);
        let (second, _) = watch::channel(false);
        let keys = Mutex::new(HashMap::from([
            ("native-a".into(), ("say-1".into(), first)),
            ("native-b".into(), ("say-2".into(), second)),
        ]));
        for (native, key, heard, played) in [
            ("native-a", "say-1", 3, true),
            ("native-b", "say-2", 0, false),
        ] {
            let event = VoiceEvent::AgentInterrupted(AgentInterrupted {
                utterance_id: native.into(),
                heard_until: heard,
                played,
            });
            let (value, sent) = event_for_page(&event, &keys).unwrap();
            assert_eq!(value, json!({"type": "agent-interrupted", "data": {
                "utterance_id": key, "heard_until": heard, "played": played
            }}));
            assert!(sent.is_some());
            assert!(event_for_page(&event, &keys).is_none(), "one report per native say");
        }
    }
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
