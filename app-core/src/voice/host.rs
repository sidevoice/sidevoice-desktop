//! The native voice host's calls (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice call"), apart from
//! the app: one lane of the shared microphone and speaker per call, the call on it (sidevoice-voice's `VoiceCall`,
//! which loads only the halves it has), what it tells the page, and what a stopped call or a preload keeps loaded for
//! its idle minutes. The commands that reach it from the page, and the engine it takes its models from, are in
//! `voice.rs`.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use super::meters::Meters;
use super::{self as choice, VoiceSettings};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use sidevoice_voice::{
    AudioIo, Events, InputPort, Listening, NativeDevices, SayCancel, SayEvent, SayOptions, SayOutcome,
    SharedNativeAudio, StartOptions, StopReason, Take, VoiceCall, VoiceEvent,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::models::{Halves, ModelCache};

#[cfg(test)]
mod tests;

/// The keeps (`VoiceState::keep`) still waiting to let go, for CI's memory run.
#[cfg(feature = "probe")]
static PENDING_KEEPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How long the models of a stopped call, or of a preload, stay loaded when the settings do not say, in minutes.
pub const IDLE_UNLOAD_MINUTES: u32 = 10;

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
pub type Page = Arc<dyn Fn(Value) + Send + Sync>;

/// What a call runs now: the person's settings and the halves the app made from them.
#[derive(Clone)]
pub struct Applied {
    pub settings: VoiceSettings,
    pub halves: Halves,
}

impl Applied {
    /// How long the models stay loaded once nothing holds them, in minutes.
    pub fn idle_minutes(&self) -> u32 {
        self.settings.idle_unload_minutes.unwrap_or(IDLE_UNLOAD_MINUTES)
    }
}

/// One native host owns the devices and the loaded model instances. Every call has its own audio lane, VAD stream,
/// recognition state, playback, and event stream.
pub struct VoiceState<A: LaneAudio = SharedNativeAudio> {
    calls: tokio::sync::Mutex<HashMap<String, Arc<CallLane<A::Port>>>>,
    audio: A,
    pub model_cache: Arc<ModelCache>,
    /// The lane the microphone is routed to, by the lane itself and not by its key: once a lane has gone, a newer page can
    /// make a lane under the same key, and what clears the route of the one that went must not clear the new one's.
    route: tokio::sync::Mutex<Option<Weak<CallLane<A::Port>>>>,
    pub devices: NativeDevices,
    pub watching: AtomicBool,
    /// The generation of the current page: the one place that counts them (the host's sinks read it too). A lane or a
    /// route is made for a page only while it is the current one, checked under the lock that makes it.
    pub page: Arc<AtomicU64>,
    /// How long an idle minute is: a minute.
    pub minute: Duration,
    /// The tasks keeping a stopped call's or a preload's models for their idle minutes.
    keeping: Mutex<Vec<JoinHandle<()>>>,
    /// Quitting: nothing is kept any more; what would be is let go at once.
    closing: AtomicBool,
}

pub struct CallLane<P> {
    /// The generation of the page that made the lane (`VoiceState::create_call_in`): a page owns its lanes only.
    page: u64,
    call: tokio::sync::Mutex<Option<Running>>,
    /// Held while a change of the call's settings is made, so that changes are made one after the other, in the order
    /// they were asked.
    pub changing: tokio::sync::Mutex<()>,
    io: Mutex<Option<Box<dyn AudioIo>>>,
    input: P,
    /// What the page last asked of `mute`, kept from the lane's creation: the call is made with it.
    muted: AtomicBool,
    lifecycle: Arc<Mutex<Lifecycle>>,
    /// Whether the lane was released; held while an event is handed to the page, so that once it is set nothing
    /// reaches the page again (see `CallLane::release`).
    released: Arc<Mutex<bool>>,
    sayings: Arc<Mutex<HashMap<String, SayCancel>>>,
    /// The tasks handing each saying's steps to the page, up to its `done`.
    telling: Mutex<Vec<JoinHandle<()>>>,
}

/// The lane's call, and what it was made of.
struct Running {
    call: VoiceCall,
    applied: Applied,
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

fn not_started() -> Refusal {
    refusal("voice_not_started", "The call has not started.")
}

/// `voice_failed` with the call's own `code`, after `what`.
pub fn failed(what: &str, code: String) -> Refusal {
    let mut refused = refusal("voice_failed", format!("{what}: {code}."));
    refused.params.insert("code".into(), json!(code));
    refused
}

impl<A: LaneAudio> VoiceState<A> {
    /// How long an idle minute is: a minute; CI's probe build can shorten it.
    fn idle_minute() -> Duration {
        #[cfg(feature = "probe")]
        {
            // With `SIDEVOICE_DEBUG_IDLE_MINUTE_MS`, how long it is: the models of a stopped call stay loaded for
            // `idle_unload_minutes` of them, so `probe memory` can wait for them to be let go.
            let millis = std::env::var("SIDEVOICE_DEBUG_IDLE_MINUTE_MS").ok().and_then(|v| v.parse::<u64>().ok());
            if let Some(millis) = millis.filter(|&m| crate::debug::enabled() && m > 0) {
                return Duration::from_millis(millis);
            }
        }
        Duration::from_secs(60)
    }

    pub fn new(audio: A, devices: NativeDevices) -> Self {
        Self {
            calls: tokio::sync::Mutex::new(HashMap::new()),
            audio,
            model_cache: Arc::new(ModelCache::default()),
            route: tokio::sync::Mutex::new(None),
            devices,
            watching: AtomicBool::new(false),
            page: Arc::default(),
            // CI's memory run shortens the minute so that the models' release is waited for (`idle_minute`).
            minute: Self::idle_minute(),
            keeping: Mutex::new(Vec::new()),
            closing: AtomicBool::new(false),
        }
    }

    /// Refuses what is for a page that is not the current one.
    fn current(&self, page: u64) -> Result<(), Refusal> {
        if self.page.load(Ordering::SeqCst) == page {
            Ok(())
        } else {
            Err(refusal("page_stale", "That page has been replaced by a newer one."))
        }
    }

    pub async fn lane(&self, key: &str) -> Result<Arc<CallLane<A::Port>>, Refusal> {
        self.calls.lock().await.get(key).cloned().ok_or_else(|| refusal("bad_request", "Unknown voice call."))
    }

    /// Whether the microphone is routed to this very lane.
    fn routes_to(route: &Option<Weak<CallLane<A::Port>>>, lane: &Arc<CallLane<A::Port>>) -> bool {
        route.as_ref().is_some_and(|routed| std::ptr::eq(routed.as_ptr(), Arc::as_ptr(lane)))
    }

    /// Takes `lane` out of the lanes, if it is still the one under `key`: a lane a newer page has made under the same
    /// key since is not this one's to take out.
    async fn remove(&self, key: &str, lane: &Arc<CallLane<A::Port>>) {
        let mut calls = self.calls.lock().await;
        if calls.get(key).is_some_and(|held| Arc::ptr_eq(held, lane)) {
            calls.remove(key);
        }
    }

    /// A lane for the current page.
    pub async fn create_call(&self, key: String) -> Result<(), Refusal> {
        self.create_call_in(key, self.page.load(Ordering::SeqCst)).await
    }

    /// A lane made by the page of generation `page`, if it is still the current one when the lane is inserted: the check
    /// is under the lock the lane is inserted with, and `clear_older_than` (which comes after a page changed) takes the
    /// same, so a lane of a page that has gone is neither made nor left behind. `clear_older_than` lets go of it once a
    /// newer page exists.
    pub async fn create_call_in(&self, key: String, page: u64) -> Result<(), Refusal> {
        if !valid_key(&key) {
            return Err(refusal("bad_request", "Invalid voice call key."));
        }
        let mut calls = self.calls.lock().await;
        self.current(page)?;
        if calls.contains_key(&key) {
            return Err(refusal("bad_request", "Voice call key already exists."));
        }
        let (io, input) = self.audio.attach();
        calls.insert(key, Arc::new(CallLane::new(io, input, page)));
        Ok(())
    }

    pub async fn route_input(&self, key: Option<String>) -> Result<(), Refusal> {
        self.route_input_in(self.page.load(Ordering::SeqCst), key).await
    }

    /// The route, for the page of generation `page`, if it is still the current one once the route is held: an old page
    /// that is late cannot silence the new one.
    pub async fn route_input_in(&self, page: u64, key: Option<String>) -> Result<(), Refusal> {
        let mut route = self.route.lock().await;
        self.current(page)?;
        let target = match key.as_deref() {
            Some(key) => Some(self.lane(key).await?),
            None => None,
        };
        // A page routes to its own lanes: an older page's lane that is still being cleared away is not one to route to.
        if target.as_ref().is_some_and(|lane| lane.page != page) {
            return Err(refusal("bad_request", "Unknown voice call."));
        }
        self.audio
            .route_input(target.as_ref().map(|lane| &lane.input))
            .await
            .map_err(|code| failed("The microphone route failed", code))?;
        *route = target.as_ref().map(Arc::downgrade);
        Ok(())
    }

    /// Keeps `held` for `minutes` of the host's idle minutes, then lets it go (at once for none, or when quitting).
    fn keep<T: Send + 'static>(&self, held: T, minutes: u32) {
        if minutes == 0 || self.closing.load(Ordering::SeqCst) {
            return;
        }
        let wait = self.minute.saturating_mul(minutes);
        #[cfg(feature = "probe")]
        PENDING_KEEPS.fetch_add(1, Ordering::SeqCst);
        let task = tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            drop(held);
            #[cfg(feature = "probe")]
            PENDING_KEEPS.fetch_sub(1, Ordering::SeqCst);
        });
        let mut keeping = self.keeping.lock().unwrap();
        keeping.retain(|task| !task.is_finished());
        keeping.push(task);
    }

    /// CI's memory run (src/probe.rs): what the host holds now, as `keeps=N lanes=M`.
    #[cfg(feature = "probe")]
    pub fn held(&self) -> String {
        let lanes = self.calls.try_lock().map_or("busy".to_owned(), |calls| calls.len().to_string());
        format!("keeps={} lanes={lanes}", PENDING_KEEPS.load(Ordering::SeqCst))
    }

    /// Lets go of the lane's call, a stopped one: it keeps the models it loaded for its idle minutes, so that another
    /// call (or a preload) asking for the same ones meanwhile shares them instead of loading them again.
    async fn retire(&self, lane: &CallLane<A::Port>) {
        if let Some(running) = lane.call.lock().await.take() {
            let minutes = running.applied.idle_minutes();
            self.keep(running, minutes);
        }
    }

    /// Stops the call, what it still had to tell handed to the page (its sayings' ends, the end of an open turn, its
    /// take), then releases its lane: the last lane closes the devices. The models it loaded stay for its idle minutes.
    pub async fn dispose_call(&self, key: &str) -> Result<(), Refusal> {
        let Some(lane) = self.calls.lock().await.get(key).cloned() else { return Ok(()) };
        {
            let mut route = self.route.lock().await;
            if Self::routes_to(&route, &lane) {
                self.audio.route_input(None).await.map_err(|code| refusal("voice_failed", code))?;
                *route = None;
            }
        }
        lane.stop().await;
        self.remove(key, &lane).await;
        lane.release();
        self.retire(&lane).await;
        Ok(())
    }

    /// Makes the lane's call of `applied`, muted as the page last asked, its events handed to `page` under `key`, and
    /// starts it: it loads the models its halves use, opens the speaker and, with an input half, the microphone, and
    /// listens (`ready` says when). With `take`, the call keeps what the microphone gives from then until that
    /// listening ends, and hands it to the page then.
    pub async fn open(&self, key: &str, applied: Applied, take: bool, page: Page) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        let mut running = lane.call.lock().await;
        if running.is_some() {
            return Err(refusal("bad_request", "The voice call has started already."));
        }
        let setup = applied.halves.setup().map_err(|code| refusal("internal", code))?;
        let io =
            lane.io.lock().unwrap().take().ok_or_else(|| refusal("internal", "Voice lane audio was already taken."))?;
        let (call, events) = VoiceCall::new(io, setup);
        call.mute(lane.muted.load(Ordering::SeqCst));
        forward(page, key.to_owned(), events, Arc::clone(&lane.lifecycle), Arc::clone(&lane.released));
        call.start(StartOptions { take });
        *running = Some(Running { call, applied });
        Ok(())
    }

    /// What the lane's call runs now.
    pub async fn applied(&self, key: &str) -> Result<Applied, Refusal> {
        let lane = self.lane(key).await?;
        let running = lane.call.lock().await;
        running.as_ref().map(|running| running.applied.clone()).ok_or_else(not_started)
    }

    /// Changes the lane's call to `applied` without restarting it (sidevoice-voice's `update`): a half whose models
    /// are the same keeps what it loaded, a change of models loads them while the call goes on as it was, and each
    /// half's change takes effect at its safe point. Resolves once the change is accepted.
    pub async fn update(&self, key: &str, applied: Applied) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        let mut running = lane.call.lock().await;
        let running = running.as_mut().ok_or_else(not_started)?;
        let setup = applied.halves.setup().map_err(|code| refusal("internal", code))?;
        running.call.update(setup);
        running.applied = applied;
        Ok(())
    }

    /// Resolves once the call works: at its first state that is not idle (`off` for a call without an input half).
    /// Rejects with the code of why it does not (a model that cannot load, the devices refused, ...), or `stopped`
    /// when a stop came first.
    pub async fn ready(&self, key: &str) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        let waiting = {
            let mut lifecycle = lane.lifecycle.lock().unwrap();
            match &lifecycle.outcome {
                Some(Ok(())) => return Ok(()),
                Some(Err(code)) => return Err(failed("The call could not start", code.clone())),
                None => {}
            }
            let (sender, waiting) = oneshot::channel();
            lifecycle.waiting.push(sender);
            waiting
        };
        match waiting.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(code)) => Err(failed("The call could not start", code)),
            Err(_) => Err(refusal("internal", "the call ended")),
        }
    }

    /// Loads the models `halves` use, ahead of a call, and keeps them for `minutes` from the end of the load: a call
    /// that starts on the same models meanwhile takes them without loading. What loaded stays when a slot did not
    /// load; the code of that one is the error.
    pub async fn preload(&self, halves: Halves, minutes: u32) -> Result<(), String> {
        let (loaded, failed) = halves.load().await;
        self.keep(loaded, minutes);
        failed.map_or(Ok(()), Err)
    }

    /// Stops the call's listening and speaking; the lane stays until it is disposed. Returns once the idle state that
    /// answers the stop has reached the page and every saying under way has told its end; false when there is no call.
    pub async fn stop(&self, key: &str) -> Result<bool, Refusal> {
        Ok(self.lane(key).await?.stop().await)
    }

    pub async fn say(
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
        let running = lane.call.lock().await;
        let Some(Running { call, .. }) = running.as_ref() else {
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

    pub async fn cancel_say(&self, key: &str, say_key: &str) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        if let Some(cancel) = lane.sayings.lock().unwrap().get(say_key) {
            cancel.cancel();
        }
        Ok(())
    }

    /// Kept for the call from the lane's creation on, and applied to it once it exists.
    pub async fn mute(&self, key: &str, muted: bool) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        lane.muted.store(muted, Ordering::SeqCst);
        if let Some(running) = lane.call.lock().await.as_ref() {
            running.call.mute(lane.muted.load(Ordering::SeqCst));
        }
        Ok(())
    }

    /// Before the call there is nothing to cancel.
    pub async fn cancel_input(&self, key: &str) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        if let Some(running) = lane.call.lock().await.as_ref() {
            running.call.cancel_input();
        }
        Ok(())
    }

    /// Closes the call's open turn now with what was captured so far; resolves once it is closed (not transcribed).
    /// With no call yet, nothing open, or the call stopped, nothing happens and it resolves.
    pub async fn end_input(&self, key: &str) -> Result<(), Refusal> {
        let lane = self.lane(key).await?;
        let closed = match lane.call.lock().await.as_ref() {
            Some(running) => running.call.end_input(),
            None => return Ok(()),
        };
        closed.await;
        Ok(())
    }

    /// Quitting: no route and no lanes (the last lane closes the devices), nothing kept for its idle minutes any more,
    /// then waits, every `poll`, for no model of the engine to be held (a dropped call's own task ends after it; a
    /// transcription under way finishes). Returns the models still held when `limit` passed, by their key; none once
    /// every one was released.
    pub async fn close(&self, limit: Duration, poll: Duration) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + limit;
        self.closing.store(true, Ordering::SeqCst);
        if tokio::time::timeout_at(deadline, self.clear()).await.is_err() {
            return vec!["(the calls did not stop)".into()];
        }
        let keeping = std::mem::take(&mut *self.keeping.lock().unwrap());
        for task in &keeping {
            task.abort();
        }
        // An aborted task's join answers once its future, and what it kept, was dropped.
        for task in keeping {
            let _ = task.await;
        }
        loop {
            let held = self.model_cache.held();
            if held.is_empty() || tokio::time::Instant::now() >= deadline {
                return held;
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// The lanes of pages older than `page` are stopped and released, as `clear` does, and the route is let go if it led to
    /// one of them; the lanes of `page` and of newer ones, and a route to them, are left alone. The newest page may have
    /// made its lanes before this ran, under the key of one that went too: the route is to a lane, not to a key, so a
    /// route the newer page made to its own is not the old one's to clear.
    pub async fn clear_older_than(&self, page: u64) {
        let old: Vec<(String, Arc<CallLane<A::Port>>)> = {
            let mut calls = self.calls.lock().await;
            let keys: Vec<String> =
                calls.iter().filter(|(_, lane)| lane.page < page).map(|(key, _)| key.clone()).collect();
            keys.into_iter().filter_map(|key| calls.remove(&key).map(|lane| (key, lane))).collect()
        };
        // What waits on these lanes hears first that they are over, whatever their calls are doing (one still loading its
        // models may not answer a stop for as long as it loads), and nothing they say reaches the page any more.
        for (_, lane) in &old {
            lane.release();
        }
        {
            let mut route = self.route.lock().await;
            if old.iter().any(|(_, lane)| Self::routes_to(&route, lane)) {
                let _ = self.audio.route_input(None).await;
                *route = None;
            }
        }
        for (_, lane) in old {
            lane.stop().await;
            self.retire(&lane).await;
        }
    }

    /// No route and no lanes: each old call is stopped and released, and keeps its models for its idle minutes.
    pub async fn clear(&self) {
        {
            let mut route = self.route.lock().await;
            let _ = self.audio.route_input(None).await;
            *route = None;
        }
        let old = std::mem::take(&mut *self.calls.lock().await);
        for lane in old.values() {
            lane.release();
        }
        for lane in old.into_values() {
            lane.stop().await;
            self.retire(&lane).await;
        }
    }
}

impl<P> CallLane<P> {
    fn new(io: Box<dyn AudioIo>, input: P, page: u64) -> Self {
        Self {
            page,
            call: tokio::sync::Mutex::new(None),
            changing: tokio::sync::Mutex::new(()),
            io: Mutex::new(Some(io)),
            input,
            muted: AtomicBool::new(false),
            lifecycle: Arc::new(Mutex::new(Lifecycle::default())),
            released: Arc::new(Mutex::new(false)),
            sayings: Arc::new(Mutex::new(HashMap::new())),
            telling: Mutex::new(Vec::new()),
        }
    }

    /// The lane is gone: nothing more reaches the page from it. Dropping the call makes its task close the call once
    /// more (a last playback level and an idle state) after `stop` has answered, at a time of its own; those belong
    /// to no one. Waits for the event being handed over, if any.
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        // Whatever still waits on the call hears that it is over: a `ready` of a call that never worked, a stop whose idle
        // state will not come.
        self.lifecycle.lock().unwrap().end();
    }

    /// Stops listening and speaking; the models stay loaded. A `ready` still waiting rejects with `stopped`. Returns
    /// once the idle state that answers the stop has reached the page (the take, and the end of an open turn, before
    /// it) and every saying under way has told its end; false when there is no call yet.
    async fn stop(&self) -> bool {
        let (answered, telling) = {
            let running = self.call.lock().await;
            let Some(Running { call, .. }) = running.as_ref() else { return false };
            let (sender, answered) = oneshot::channel();
            {
                let mut lifecycle = self.lifecycle.lock().unwrap();
                lifecycle.settle(Err("stopped".into()));
                // A lane that was released has told its waiters it is over: this stop waits for nothing, since the idle state
                // that would answer it may never come (a call still loading its models).
                if !*self.released.lock().unwrap() {
                    lifecycle.stopping += 1;
                    lifecycle.stopped.push_back(sender);
                }
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

/// Where the call is: how it came to work, the `ready`s waiting to hear that it does (or why it does not), and the
/// stops waiting for the idle state that answers each.
#[derive(Default)]
struct Lifecycle {
    /// Once the call works (its first state that is not idle) or fails before that (or is stopped): how. Kept, so that
    /// a `ready` asked after it still hears it.
    outcome: Option<Result<(), String>>,
    /// Stops whose idle state has not come back: the states before it are the old listening's.
    stopping: usize,
    waiting: Vec<oneshot::Sender<Result<(), String>>>,
    stopped: VecDeque<oneshot::Sender<()>>,
}

impl Lifecycle {
    fn settle(&mut self, result: Result<(), String>) {
        if self.outcome.is_none() {
            self.outcome = Some(result.clone());
        }
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
        if !idle {
            self.settle(Ok(()));
        }
        None
    }

    fn error(&mut self, code: &str) {
        if self.stopping == 0 {
            self.settle(Err(code.to_owned()));
        }
    }

    /// The call is gone: nothing waiting on it will be answered by it.
    fn end(&mut self) {
        self.settle(Err("stopped".into()));
        self.stopped.clear();
    }
}

/// A refusal, in the shape of the bridge's (docs/BRIDGE.md → "Refusals"): a stable key, its parameters, the English
/// sentence for a page that does not know the key. Each carries `code` too, the seam's (sidevoice-voice
/// `js/voice-host.d.ts`): the call's own code when it failed, else the key's, without `voice_` and with dashes.
#[derive(Debug, Serialize)]
pub struct Refusal {
    key: String,
    #[serde(flatten)]
    pub params: serde_json::Map<String, Value>,
    message: String,
}

pub fn refusal(key: &str, message: impl Into<String>) -> Refusal {
    let mut params = serde_json::Map::new();
    params.insert("code".into(), json!(key.trim_start_matches("voice_").replace('_', "-")));
    Refusal { key: key.into(), params, message: message.into() }
}

/// A configuration the app cannot make, as the bridge refuses it: the code the page translates (`halves-missing`,
/// `model-unknown`, `model-unfit`, `catalog-not-found`, `build-not-found`, `vad-unavailable`,
/// `end-of-turn-unavailable`, or the engine's own, such as `credential-missing` or why a chosen build does not run),
/// the model, and what a provider said (`detail`).
pub fn refused(r: choice::Refusal) -> Refusal {
    let mut refused = refusal(&format!("voice_{}", r.code.replace('-', "_")), r.message);
    refused.params.insert("code".into(), json!(r.code));
    refused.params.insert("model".into(), json!(r.model));
    if let Some(detail) = r.detail {
        refused.params.insert("detail".into(), json!(detail));
    }
    refused
}

/// Forward events with their immutable call key. Asynchronous delivery never consults the current input route. The
/// idle state that answers a stop answers it once it is handed to the page. The meters go only when the page would draw
/// them differently (`voice::meters`): about 60 a second otherwise, silence included (desktop#149).
fn forward(page: Page, key: String, mut events: Events, lifecycle: Arc<Mutex<Lifecycle>>, released: Arc<Mutex<bool>>) {
    tokio::spawn(async move {
        let mut meters = Meters::default();
        while let Some(event) = events.next().await {
            let wanted = match &event {
                VoiceEvent::Level(level) => meters.level(*level),
                VoiceEvent::PlaybackLevel(level) => meters.playback(*level),
                _ => true,
            };
            if !wanted {
                continue;
            }
            let mut value = match &event {
                VoiceEvent::Take(take) => {
                    crate::debug::debug(&format!(
                        "voice take: {} samples, {} turns",
                        take.samples.len(),
                        take.turns.len()
                    ));
                    page_take(take)
                }
                _ => {
                    if !matches!(event, VoiceEvent::Level(_) | VoiceEvent::PlaybackLevel(_)) {
                        crate::debug::debug(&format!(
                            "voice event {}",
                            serde_json::to_string(&event).unwrap_or_default()
                        ));
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
            // Where the call is, once the page has the event that says so: what waits on it hears it after the page.
            let answered = match &event {
                VoiceEvent::State(state) => lifecycle.lock().unwrap().state(state.listening == Listening::Idle),
                VoiceEvent::Error(error) => {
                    lifecycle.lock().unwrap().error(&error.code);
                    None
                }
                _ => None,
            };
            if let Some(answered) = answered {
                let _ = answered.send(());
            }
        }
        lifecycle.lock().unwrap().end();
    });
}

/// A take as the page gets it (`voice::take`).
fn page_take(take: &Take) -> Value {
    choice::take::page_event(
        take.sample_rate,
        &take.samples,
        take.started_at,
        take.ended_at,
        serde_json::to_value(&take.turns).unwrap_or_default(),
    )
}
