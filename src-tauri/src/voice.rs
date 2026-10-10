//! The voice call, run by the app itself (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"):
//! sidevoice-voice's `VoiceCall` with the models the app picks on its engine (`models`), the device's own
//! microphone and speaker and WebRTC's echo cancellation between them (`NativeIo`). The page keeps the room, which the
//! call knows nothing of: it tells the room the call's turns, and has the call say what the room sends (`voice_say`),
//! each through a handle in the page that tells how it went.
//!
//! Only the room window's own page may call these (capabilities/room.json, and `room_page`). The call's events reach
//! that page as `window.__sidevoiceDesktop.voiceEvent(event)`, each sidevoice-voice's `VoiceEvent` as JSON, and each
//! step of something said as `{type: "say", data: {key, event}}`. A page that loads anew finds the call stopped, its
//! models still loaded.
//!
//! macOS only: the echo canceller's C++ does not build on Windows yet, and the beta ships on macOS. Elsewhere the page
//! runs the call itself.
#![cfg(target_os = "macos")]

mod models;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use models::EngineModels;
use serde::Serialize;
use serde_json::{json, Value};
use sidevoice_desktop_core::voice::{self as choice, choose, Candidate, CatalogCandidates, VoiceChoice, VoiceSettings};
use sidevoice_desktop_engine::{CatalogView, NativeEngines};
use sidevoice_voice::{
    Events, Listening, NativeIo, SayCancel, SayEvent, SayOptions, SayOutcome, StopReason, VoiceCall, VoiceConfig,
    VoiceEvent,
};
use tauri::{AppHandle, Manager, State, Webview};
use tokio::sync::oneshot;

use crate::engine_ipc::EngineState;

/// The call, once the page has set its settings, with the models and configuration it was given last.
#[derive(Default)]
pub struct VoiceState {
    call: tokio::sync::Mutex<Option<(VoiceCall, VoiceChoice)>>,
    lifecycle: Arc<Mutex<Lifecycle>>,
    /// What is being said, by the page's key for it, until it is done.
    sayings: Arc<Mutex<HashMap<String, SayCancel>>>,
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

/// A configuration the app cannot make, as the bridge refuses it: the code the page translates (`model-unknown`,
/// `model-unfit`, `catalog-not-found`, `vad-unavailable`, `end-of-turn-unavailable`, or the engine's own, such as
/// `credential-missing`), the model, and what a provider said (`detail`).
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
/// person does not choose). The call takes both at once: other models come with their configuration, and a call that
/// listens restarts once on the pair. The first settings create the call.
#[tauri::command]
pub async fn voice_set_settings(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    settings: VoiceSettings,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
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
    let mut saying = call.say(text, SayOptions { language });
    state.sayings.lock().unwrap().insert(key.clone(), saying.canceller());
    let sayings = Arc::clone(&state.sayings);
    tauri::async_runtime::spawn(async move {
        while let Some(event) = saying.next().await {
            if matches!(event, SayEvent::Done { .. }) {
                sayings.lock().unwrap().remove(&key);
            }
            to_page(&app, &json!({"type": "say", "data": {"key": key, "event": event}}));
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
            if let Ok(event) = serde_json::to_value(&event) {
                to_page(&app, &event);
            }
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
/// provider's always does.
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
            })
            .collect(),
    }
}
