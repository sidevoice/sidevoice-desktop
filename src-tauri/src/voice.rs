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
#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::future::Future;
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
use tokio::task::JoinHandle;

use crate::engine_ipc::EngineState;

/// The microphone and the speaker every call's lane is attached to: the device's own (`SharedNativeAudio`), or a
/// stand-in.
pub trait LaneAudio: Send + Sync + 'static {
    /// What routes the microphone to one lane.
    type Port: Send + Sync + 'static;
    /// A new lane: one call's own audio, and the port that routes the microphone to it.
    fn attach(&self) -> (Box<dyn AudioIo>, Self::Port);
    /// Routes the microphone to `target`'s lane from the next frame on, silence to every other; `None` silences all.
    fn route_input(&self, target: Option<&Self::Port>) -> impl Future<Output = Result<(), String>> + Send;
}

impl LaneAudio for SharedNativeAudio {
    type Port = InputPort;

    fn attach(&self) -> (Box<dyn AudioIo>, InputPort) {
        SharedNativeAudio::attach(self)
    }

    fn route_input(&self, target: Option<&InputPort>) -> impl Future<Output = Result<(), String>> + Send {
        SharedNativeAudio::route_input(self, target)
    }
}

/// Where the call's events go: the room page (`to_page`), as JSON.
type Page = Arc<dyn Fn(Value) + Send + Sync>;

/// One native host owns the devices and the loaded model instances. Every call has its own
/// audio lane, VAD stream, recognition state, playback, and event stream.
pub struct VoiceState<A: LaneAudio = SharedNativeAudio> {
    calls: tokio::sync::Mutex<HashMap<String, Arc<CallLane<A::Port>>>>,
    audio: A,
    model_cache: Arc<ModelCache>,
    route: tokio::sync::Mutex<Option<String>>,
    devices: NativeDevices,
    watching: AtomicBool,
}

struct CallLane<P> {
    call: tokio::sync::Mutex<Option<(VoiceCall, Arc<EngineModels>)>>,
    io: Mutex<Option<Box<dyn AudioIo>>>,
    input: P,
    /// What the page last asked of `mute`, kept from the lane's creation: the call is made with it.
    muted: AtomicBool,
    settings: Arc<AtomicU64>,
    lifecycle: Arc<Mutex<Lifecycle>>,
    /// Whether the lane was released; held while an event is handed to the page, so that once it is set nothing
    /// reaches the page again (see `CallLane::release`).
    released: Arc<Mutex<bool>>,
    sayings: Arc<Mutex<HashMap<String, SayCancel>>>,
    /// The tasks handing each saying's steps to the page, up to its `done`.
    telling: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for VoiceState {
    fn default() -> Self {
        let devices = NativeDevices::default();
        Self::new(SharedNativeAudio::new(devices.clone()), devices)
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn settings_missing() -> Refusal {
    refusal("voice_settings_missing", "Set the voice settings first.")
}

/// `voice_failed` with the call's own `code`, after `what`.
fn failed(what: &str, code: String) -> Refusal {
    let mut refused = refusal("voice_failed", format!("{what}: {code}."));
    refused.params.insert("code".into(), json!(code));
    refused
}

impl<A: LaneAudio> VoiceState<A> {
    fn new(audio: A, devices: NativeDevices) -> Self {
        Self {
            calls: tokio::sync::Mutex::new(HashMap::new()),
            audio,
            model_cache: Arc::new(ModelCache::default()),
            route: tokio::sync::Mutex::new(None),
            devices,
            watching: AtomicBool::new(false),
        }
    }

    async fn lane(&self, key: &str) -> Result<Arc<CallLane<A::Port>>, Refusal> {
        self.calls.lock().await.get(key).cloned().ok_or_else(|| refusal("bad_request", "Unknown voice call."))
    }

    async fn create_call(&self, key: String) -> Result<(), Refusal> {
        if !valid_key(&key) {
            return Err(refusal("bad_request", "Invalid voice call key."));
        }
        let mut calls = self.calls.lock().await;
        if calls.contains_key(&key) {
            return Err(refusal("bad_request", "Voice call key already exists."));
        }
        let (io, input) = self.audio.attach();
        calls.insert(key, Arc::new(CallLane::new(io, input)));
        Ok(())
    }

    async fn route_input(&self, key: Option<String>) -> Result<(), Refusal> {
        let mut route = self.route.lock().await;
        let target = match key.as_deref() {
            Some(key) => Some(self.lane(key).await?),
            None => None,
        };
        self.audio
            .route_input(target.as_ref().map(|lane| &lane.input))
            .await
            .map_err(|code| failed("The microphone route failed", code))?;
        *route = key;
        Ok(())
    }

    /// Stops the call, what it still had to tell handed to the page (its sayings' ends, the end of an open turn, its
    /// take), then releases its lane: the last lane closes the devices.
    async fn dispose_call(&self, key: &str) -> Result<(), Refusal> {
        {
            let mut route = self.route.lock().await;
            if route.as_deref() == Some(key) {
                self.audio.route_input(None).await.map_err(|code| refusal("voice_failed", code))?;
                *route = None;
            }
        }
        let Some(lane) = self.calls.lock().await.get(key).cloned() else { return Ok(()) };
        lane.stop().await;
        self.calls.lock().await.remove(key);
        lane.release();
        Ok(())
    }

    /// Resolves once the call listens (at once if it does and no stop is under way; with the first when one is
    /// pending). A start after a stop asks the call to start again, whether or not that stop's idle has come back.
    async fn start(&self, key: &str, take: bool) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        let started = {
            let call = lane.call.lock().await;
            let (call, _) = call.as_ref().ok_or_else(settings_missing)?;
            let mut lifecycle = lane.lifecycle.lock().unwrap();
            if lifecycle.listening && lifecycle.stopping == 0 {
                return Ok(());
            }
            let (sender, started) = oneshot::channel();
            lifecycle.waiting.push(sender);
            // A start already asked keeps its take: a second start changes nothing.
            if !lifecycle.active {
                lifecycle.active = true;
                call.start(StartOptions { take });
            }
            started
        };
        match started.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(code)) => Err(failed("The call could not start", code)),
            Err(_) => Err(refusal("internal", "the call ended")),
        }
    }

    async fn stop(&self, key: &str) -> Result<bool, Refusal> {
        Ok(self.lane(key).await?.stop().await)
    }

    async fn say(
        &self,
        key: &str,
        say_key: String,
        text: String,
        language: Option<String>,
        page: Page,
    ) -> Result<(), Refusal> {
        if !valid_key(&say_key) {
            return Err(refusal("bad_request", "Invalid say key."));
        }
        let lane = self.lane(key).await?;
        let call = lane.call.lock().await;
        let Some((call, _)) = call.as_ref() else {
            let stopped = SayEvent::Done { outcome: SayOutcome::NotPlayed { reason: StopReason::Stopped } };
            page(json!({"key": key, "type": "say", "data": {"key": say_key, "event": stopped}}));
            return Ok(());
        };
        let mut saying = call.say(text, SayOptions { language });
        lane.sayings.lock().unwrap().insert(say_key.clone(), saying.canceller());
        let sayings = Arc::clone(&lane.sayings);
        let key = key.to_owned();
        let telling = tokio::spawn(async move {
            while let Some(event) = saying.next().await {
                if matches!(event, SayEvent::Done { .. }) {
                    sayings.lock().unwrap().remove(&say_key);
                }
                page(json!({"key": key, "type": "say", "data": {"key": say_key, "event": event}}));
            }
        });
        let mut tasks = lane.telling.lock().unwrap();
        tasks.retain(|task| !task.is_finished());
        tasks.push(telling);
        Ok(())
    }

    async fn cancel_say(&self, key: &str, say_key: &str) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        if let Some(cancel) = lane.sayings.lock().unwrap().get(say_key) {
            cancel.cancel();
        }
        Ok(())
    }

    /// Kept for the call from the lane's creation on, and applied to it once it exists.
    async fn mute(&self, key: &str, muted: bool) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        lane.muted.store(muted, Ordering::SeqCst);
        if let Some((call, _)) = lane.call.lock().await.as_ref() {
            call.mute(lane.muted.load(Ordering::SeqCst));
        }
        Ok(())
    }

    /// Before the settings there is nothing to cancel.
    async fn cancel_input(&self, key: &str) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        if let Some((call, _)) = lane.call.lock().await.as_ref() {
            call.cancel_input();
        }
        Ok(())
    }

    /// Closes the call's open turn now with what was captured so far; resolves once it is closed (not transcribed).
    /// With no call yet, nothing open, or the call stopped, nothing happens and it resolves.
    async fn end_input(&self, key: &str) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        let closed = match lane.call.lock().await.as_ref() {
            Some((call, _)) => call.end_input(),
            None => return Ok(()),
        };
        closed.await;
        Ok(())
    }

    /// No route and no lanes: each old call is stopped and released.
    async fn clear(&self) {
        {
            let mut route = self.route.lock().await;
            let _ = self.audio.route_input(None).await;
            *route = None;
        }
        let old = std::mem::take(&mut *self.calls.lock().await);
        for lane in old.into_values() {
            lane.stop().await;
            lane.release();
        }
    }
}

impl<P> CallLane<P> {
    fn new(io: Box<dyn AudioIo>, input: P) -> Self {
        Self {
            call: tokio::sync::Mutex::new(None),
            io: Mutex::new(Some(io)),
            input,
            muted: AtomicBool::new(false),
            settings: Arc::new(AtomicU64::new(0)),
            lifecycle: Arc::new(Mutex::new(Lifecycle::default())),
            released: Arc::new(Mutex::new(false)),
            sayings: Arc::new(Mutex::new(HashMap::new())),
            telling: Mutex::new(Vec::new()),
        }
    }

    /// Makes the lane's call on `models` and `config`, muted as the page last asked, its events handed to `page`
    /// under `key`.
    fn install(
        &self,
        call: &mut Option<(VoiceCall, Arc<EngineModels>)>,
        key: &str,
        models: Arc<EngineModels>,
        config: VoiceConfig,
        page: Page,
    ) -> Result<(), Refusal> {
        let io =
            self.io.lock().unwrap().take().ok_or_else(|| refusal("internal", "Voice lane audio was already taken."))?;
        let (created, events) = VoiceCall::new(Arc::clone(&models) as Arc<dyn VoiceModels>, io, config);
        created.mute(self.muted.load(Ordering::SeqCst));
        forward(page, key.to_owned(), events, Arc::clone(&self.lifecycle), Arc::clone(&self.released));
        *call = Some((created, models));
        Ok(())
    }

    /// The lane is gone: nothing more reaches the page from it. Dropping the call makes its task close the call once
    /// more (a last playback level and an idle state) after `stop` has answered, at a time of its own; those belong
    /// to no one. Waits for the event being handed over, if any.
    fn release(&self) {
        *self.released.lock().unwrap() = true;
    }

    /// Stops listening and speaking; the models stay loaded. A `start` still waiting rejects with `stopped`. Returns
    /// once the idle state that answers the stop has reached the page (the take, and the end of an open turn, before
    /// it) and every saying under way has told its end; false when there is no call yet.
    async fn stop(&self) -> bool {
        let (answered, telling) = {
            let call = self.call.lock().await;
            let Some((call, _)) = call.as_ref() else { return false };
            let (sender, answered) = oneshot::channel();
            {
                let mut lifecycle = self.lifecycle.lock().unwrap();
                lifecycle.settle(&Err("stopped".into()));
                lifecycle.active = false;
                lifecycle.listening = false;
                lifecycle.stopping += 1;
                lifecycle.stopped.push_back(sender);
            }
            call.stop();
            (answered, std::mem::take(&mut *self.telling.lock().unwrap()))
        };
        let _ = answered.await;
        for task in telling {
            let _ = task.await;
        }
        true
    }
}

/// How often the devices are listed to tell the page when they change: the system tells cpal of no change.
const DEVICES_EVERY: Duration = Duration::from_secs(2);

/// Where the call is: whether a start is in effect, whether it listens, the `start`s waiting to hear that it does
/// (or why it does not), and the stops waiting for the idle state that answers each.
#[derive(Default)]
struct Lifecycle {
    /// A start was asked of the call, and neither a stop nor its ending (idle, or an error before listening) came
    /// since.
    active: bool,
    listening: bool,
    /// Stops whose idle state has not come back: the states before it are the old listening's.
    stopping: usize,
    waiting: Vec<oneshot::Sender<Result<(), String>>>,
    stopped: VecDeque<oneshot::Sender<()>>,
}

impl Lifecycle {
    fn settle(&mut self, result: &Result<(), String>) {
        for waiting in self.waiting.drain(..) {
            let _ = waiting.send(result.clone());
        }
    }

    /// A state of the call's; the stop it answers, when it is the idle one a stop waits for, to answer once the page
    /// has it.
    fn state(&mut self, idle: bool) -> Option<oneshot::Sender<()>> {
        if self.stopping > 0 {
            if !idle {
                return None;
            }
            self.stopping -= 1;
            return self.stopped.pop_front();
        }
        self.listening = !idle;
        if idle {
            self.active = false;
        } else {
            self.settle(&Ok(()));
        }
        None
    }

    fn error(&mut self, code: &str) {
        if self.stopping == 0 && !self.listening {
            self.active = false;
            self.settle(&Err(code.to_owned()));
        }
    }

    /// The call is gone: nothing waiting on it will be answered by it.
    fn end(&mut self) {
        self.settle(&Err("stopped".into()));
        self.stopped.clear();
        *self = Self::default();
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

/// Attaches a lane now, so routing may be chosen before its settings or start.
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

/// Stops the call, hands the page what it still had to tell, and releases its lane.
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
    let lane = state.lane(&key).await?;
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
            let switched = models
                .switch(choice, &latest)
                .await
                .map_err(|code| failed("The new model could not be loaded", code))?;
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
            lane.install(&mut call, &key, models, config, page_sink(&app))?;
        }
    }
    Ok(())
}

/// Loads the models (installing them if they are not), opens the microphone and the speaker and listens. Resolves
/// once the call listens (at once if it does already; with the first when one is pending); rejects with the call's
/// error code, or `stopped` when `stop` comes first. With `take`, the call keeps what the microphone gives from then
/// until that listening ends, and hands it to the page then (`{type: "take", data}`); a start already asked keeps its
/// own.
#[tauri::command]
pub async fn voice_start(
    app: AppHandle,
    webview: Webview,
    state: State<'_, VoiceState>,
    take: Option<bool>,
    key: String,
) -> Result<(), Refusal> {
    page(&app, &webview)?;
    let _permit = crate::data::admit(&app).ok_or_else(|| refusal("reset.busy", "Data reset is under way."))?;
    state.start(&key, take.unwrap_or(false)).await
}

/// Stops listening and speaking; the models stay loaded. A `start` still waiting rejects with `stopped`. Answers once
/// the page has what the stop ends (`true`), or at once with `false` before the first settings.
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
/// Kept from the call's creation, before its settings too.
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

/// Cancels what the person said that is not reported yet; nothing before the settings.
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
/// nothing to close (no settings yet, no open turn, the call stopped).
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

/// A reloaded page owns none of its previous lanes.
pub fn page_changed(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<VoiceState>() {
            state.clear().await;
        }
    });
}

/// Forward events with their immutable call key. Asynchronous delivery never consults the current input route. The
/// idle state that answers a stop answers it once it is handed to the page.
fn forward(page: Page, key: String, mut events: Events, lifecycle: Arc<Mutex<Lifecycle>>, released: Arc<Mutex<bool>>) {
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let mut answered = None;
            let mut value = match &event {
                VoiceEvent::Take(take) => {
                    crate::debug(&format!("voice take: {} samples, {} turns", take.samples.len(), take.turns.len()));
                    page_take(take)
                }
                _ => {
                    match &event {
                        VoiceEvent::State(state) => {
                            answered = lifecycle.lock().unwrap().state(state.listening == Listening::Idle);
                        }
                        VoiceEvent::Error(error) => lifecycle.lock().unwrap().error(&error.code),
                        _ => {}
                    }
                    if !matches!(event, VoiceEvent::Level(_) | VoiceEvent::PlaybackLevel(_)) {
                        crate::debug(&format!("voice event {}", serde_json::to_string(&event).unwrap_or_default()));
                    }
                    serde_json::to_value(&event).unwrap_or(Value::Null)
                }
            };
            if !value.is_null() {
                value["key"] = json!(key);
                // Held across the hand-over: `release` waits for the event in hand, and nothing follows it.
                let released = released.lock().unwrap();
                if !*released {
                    page(value);
                }
            }
            if let Some(answered) = answered {
                let _ = answered.send(());
            }
        }
        lifecycle.lock().unwrap().end();
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
