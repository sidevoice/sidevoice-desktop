//! The voice call, run by the app itself (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"):
//! sidevoice-voice's `VoiceCall` with the models the app picks on its engine (`models`), the device's own
//! microphone and speaker and WebRTC's echo cancellation between them (`NativeIo`). The page keeps the room: it hands
//! the call the room's replies and carries the call's turns and playback reports to the room, in its outbox.
//!
//! Only the room window's own page may call these (capabilities/room.json, and `room_page`). The call's events reach
//! that page as `window.__sidevoiceDesktop.voiceEvent(event)`, each sidevoice-voice's `VoiceEvent` as JSON. A page
//! that loads anew finds the call stopped, its models still loaded.
//!
//! macOS only: the echo canceller's C++ does not build on Windows yet, and the beta ships on macOS. Elsewhere the page
//! runs the call itself.
#![cfg(target_os = "macos")]

mod models;

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use models::EngineModels;
use serde::Serialize;
use serde_json::{json, Value};
use sidevoice_desktop_core::voice::{choose, Candidate, CandidateBuild, VoiceChoice, VoiceSettings};
use sidevoice_desktop_engine::sidevoice_engine::{Capability, Gender, Model};
use sidevoice_desktop_engine::{accelerator_name, NativeEngines};
use sidevoice_voice::{Events, Listening, NativeIo, Reply, RoomEvent, VoiceCall, VoiceConfig, VoiceEvent};
use tauri::{AppHandle, Manager, State, Webview};
use tokio::sync::oneshot;

use crate::engine_ipc::EngineState;
use crate::keychain;

/// The call, once the page has set its settings, with the models and configuration it was given last.
#[derive(Default)]
pub struct VoiceState {
    call: tokio::sync::Mutex<Option<(VoiceCall, VoiceChoice)>>,
    lifecycle: Arc<Mutex<Lifecycle>>,
}

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

fn page(app: &AppHandle, webview: &Webview) -> Result<(), Refusal> {
    crate::room_page(app, webview).map_err(|e| refusal("bad_request", e))
}

fn engines(app: &AppHandle) -> Result<Arc<NativeEngines>, Refusal> {
    let state =
        app.try_state::<EngineState>().ok_or_else(|| refusal("engine_unavailable", "The engine did not start."))?;
    Ok(Arc::clone(&state.engines))
}

/// Sets the person's choices: the app picks the call's models from them (the builds, the detector, the end-of-turn
/// model) and makes its configuration (every number the person does not choose). The call takes both at once: other
/// models come with their configuration, and a call that listens restarts once on the pair. The first settings create
/// the call.
#[tauri::command]
pub async fn voice_set_settings(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    settings: VoiceSettings,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let engines = engines(&app)?;
    let catalogue = {
        let engines = Arc::clone(&engines);
        tauri::async_runtime::spawn_blocking(move || engines.models())
            .await
            .map_err(|e| refusal("internal", e.to_string()))?
            .map_err(|e| refusal(e.key, e.message))?
    };
    let candidates: Vec<Candidate> = catalogue.iter().map(candidate).collect();
    let choice = choose(&settings, &candidates).map_err(|r| {
        let mut refused = refusal(r.key, r.message);
        refused.params.insert("model".into(), json!(r.model));
        refused
    })?;
    let config: VoiceConfig =
        serde_json::from_value(choice.config.clone()).map_err(|e| refusal("internal", e.to_string()))?;
    crate::debug(&format!("voice settings: {}", serde_json::to_string(&choice).unwrap_or_default()));
    let models = || Arc::new(EngineModels::new(engines.engine(), choice.clone()));
    let mut call = state.call.lock().await;
    match call.as_mut() {
        Some((running, given)) => {
            if given.same_models(&choice) {
                running.set_config(config);
            } else {
                // Together: a call that listens restarts once, on the new models and the configuration they go with.
                running.set_models(models(), config);
            }
            *given = choice;
        }
        None => {
            let (created, events) = VoiceCall::new(models(), Box::new(NativeIo::new()), config);
            forward(app.clone(), events, Arc::clone(&state.lifecycle));
            *call = Some((created, choice));
        }
    }
    Ok(())
}

/// Loads the models (installing them if they are not), opens the microphone and the speaker and listens. Resolves
/// once the call listens (at once if it does already; with the first when one is pending); rejects with the call's
/// error code, or `stopped` when `stop` comes first.
#[tauri::command]
pub async fn voice_start(app: AppHandle, webview: Webview, state: State<'_, VoiceState>) -> Result<(), Refusal> {
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
        call.start();
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

/// A reply the room sent (`voice-reply`'s `data`): spoken in its turn.
#[tauri::command]
pub async fn voice_speak(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    reply: Reply,
) -> Result<(), Refusal> {
    with_call(&app, &webview, &state, |call| call.room_event(RoomEvent::Reply(reply))).await
}

/// The room's answer to a turn's `started` (`voice-user-turn`'s `data`): the call matches it by `turn_id` and drops replies
/// written before that turn.
#[tauri::command]
pub async fn voice_turn_started(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    started: Value,
) -> Result<(), Refusal> {
    let event = RoomEvent::from_json(&json!({"type": "voice-user-turn", "data": started}))
        .map_err(|e| refusal("bad_request", e.to_string()))?;
    with_call(&app, &webview, &state, |call| call.room_event(event)).await
}

/// Whether the room is in reach: turns reported while it is not say `offline`.
#[tauri::command]
pub async fn voice_set_online(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    online: bool,
) -> Result<(), Refusal> {
    with_call(&app, &webview, &state, |call| call.set_online(online)).await
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

/// The engine's catalogue, as the settings choose from it: the shape `WebEngine.models()` answers on the web.
#[tauri::command]
pub async fn voice_models(app: AppHandle, webview: Webview) -> Result<Vec<Value>, Refusal> {
    page(&app, &webview)?;
    let engines = engines(&app)?;
    let models = tauri::async_runtime::spawn_blocking(move || engines.models())
        .await
        .map_err(|e| refusal("internal", e.to_string()))?
        .map_err(|e| refusal(e.key, e.message))?;
    Ok(models.iter().map(model_value).collect())
}

/// Keeps `key` for `provider` in the keychain, or removes it when `key` is `null`.
#[tauri::command]
pub fn voice_set_provider_key(
    app: AppHandle,
    webview: Webview,
    provider: String,
    key: Option<String>,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    provider_id(&provider)?;
    let key = key.as_deref().map(str::trim).filter(|key| !key.is_empty());
    keychain::write(&provider, key).map_err(|e| refusal("keychain_failed", e))
}

/// Whether the keychain has a key for `provider`. The key itself never leaves the app.
#[tauri::command]
pub fn voice_has_provider_key(app: AppHandle, webview: Webview, provider: String) -> Result<bool, Refusal> {
    page(&app, &webview)?;
    provider_id(&provider)?;
    keychain::read(&provider).map(|key| key.is_some()).map_err(|e| refusal("keychain_failed", e))
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

fn provider_id(provider: &str) -> Result<(), Refusal> {
    if keychain::valid(provider) {
        Ok(())
    } else {
        Err(refusal("bad_request", format!("not a provider id: {provider:?}")))
    }
}

/// Hands every event of the call to the room page, in order, and keeps the lifecycle: the call listens from its first
/// state other than idle, which settles the `start`s waiting, as does the first error while they wait.
fn forward(app: AppHandle, mut events: Events, lifecycle: Arc<Mutex<Lifecycle>>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.next().await {
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
            if !matches!(event, VoiceEvent::Level(_)) {
                crate::debug(&format!("voice event {}", serde_json::to_string(&event).unwrap_or_default()));
            }
            let Ok(json) = serde_json::to_string(&event) else { continue };
            if let Some(window) = crate::room_window(&app) {
                let _ =
                    window.eval(format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.voiceEvent({json})"));
            }
        }
    });
}

fn capability(capability: &Capability) -> &'static str {
    match capability {
        Capability::Stt => "stt",
        Capability::Tts => "tts",
        Capability::Vad => "vad",
        Capability::EndOfTurn => "end-of-turn",
        _ => "other",
    }
}

fn candidate(model: &Model) -> Candidate {
    Candidate {
        id: model.id.clone(),
        capabilities: model.capabilities.iter().map(|c| capability(c).to_string()).collect(),
        builds: model
            .builds
            .iter()
            .map(|build| CandidateBuild {
                id: build.id.clone(),
                backend: build.backend.clone(),
                accelerator: build.accelerator.map(accelerator_name),
                available: build.available,
            })
            .collect(),
        recommended: model.recommended_build.clone(),
    }
}

fn model_value(model: &Model) -> Value {
    json!({
        "id": model.id,
        "family": model.family,
        "capabilities": model.capabilities.iter().map(capability).collect::<Vec<_>>(),
        "parametersM": model.parameters_m,
        "languages": model.languages,
        "license": model.license,
        "voices": model.voices.iter().map(|voice| {
            let mut value = json!({"id": voice.id, "languages": voice.languages});
            match voice.gender {
                Some(Gender::Female) => value["gender"] = json!("female"),
                Some(Gender::Male) => value["gender"] = json!("male"),
                _ => {}
            }
            value
        }).collect::<Vec<_>>(),
        "installed": model.installed,
        "builds": model.builds.iter().map(|build| json!({
            "id": build.id,
            "backend": build.backend,
            "accelerator": build.accelerator.map(accelerator_name),
            "precision": build.precision,
            "downloadBytes": build.download_bytes,
            "memoryMb": build.memory_mb,
            "available": build.available,
            "reasons": build.reasons.iter().map(|reason| {
                let mut params = serde_json::Map::new();
                if let Some(needs) = reason.needs {
                    params.insert("needs".into(), json!(needs));
                }
                if let Some(has) = reason.has {
                    params.insert("has".into(), json!(has));
                }
                json!({"code": reason.code, "params": params})
            }).collect::<Vec<_>>(),
            "installed": build.installed,
        })).collect::<Vec<_>>(),
        "recommendedBuild": model.recommended_build,
    })
}
