//! The native host's lanes, through the same methods the commands use, over a stand-in for the shared devices: one
//! microphone routed to one lane (a tone while the test asks for one, zeros to every other lane) and a speaker that
//! plays at once, or holds what it is given. Real `VoiceCall`s on the adapter's models with fake loaders; the page is
//! a list of what reached it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use sidevoice_voice::{AudioIo, IoEvent, IoSink, NativeDevices, VoiceConfig};

use super::models::tests::{chosen, config, within, FakeLoader, Held, TURN_FRAMES};
use super::models::EngineModels;
use super::{LaneAudio, Page, VoiceState};

/// The stand-in devices, as the lanes share them.
#[derive(Default)]
struct Devices {
    routed: Option<u64>,
    next: u64,
    /// Lanes attached and not released.
    attached: usize,
    /// Lanes started and not stopped: the devices are open while there is one.
    running: usize,
    /// How often the last running lane closed the devices.
    closed: usize,
    sinks: HashMap<u64, IoSink>,
}

#[derive(Clone, Default)]
struct FakeAudio {
    devices: Arc<Mutex<Devices>>,
    /// Frames of tone left for the routed lane.
    tone: Arc<AtomicUsize>,
    /// Whether the speaker holds what it is given: started, never played to its end.
    hold: Arc<AtomicBool>,
}

impl FakeAudio {
    fn speak(&self, frames: usize) {
        self.tone.store(frames, Ordering::SeqCst);
    }

    /// A device failure, as only lane `id` hears it.
    fn fail(&self, id: u64) {
        let sink = self.devices.lock().unwrap().sinks.get(&id).cloned().expect("a running lane");
        sink.send(IoEvent::Failed("audio-device-failed".into()));
    }

    fn count(&self, what: impl Fn(&Devices) -> usize) -> usize {
        what(&self.devices.lock().unwrap())
    }

    async fn until(&self, what: &str, done: impl Fn(&Devices) -> bool) {
        let waited = tokio::time::timeout(Duration::from_secs(30), async {
            while !done(&self.devices.lock().unwrap()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        waited.await.unwrap_or_else(|_| panic!("in time: {what}"));
    }
}

impl LaneAudio for FakeAudio {
    type Port = u64;

    fn attach(&self) -> (Box<dyn AudioIo>, u64) {
        let mut devices = self.devices.lock().unwrap();
        devices.next += 1;
        devices.attached += 1;
        let id = devices.next;
        (Box::new(Lane { id, audio: self.clone(), sink: None, stop: Arc::default(), started: false }), id)
    }

    fn route_input(&self, target: Option<&u64>) -> impl std::future::Future<Output = Result<(), String>> + Send {
        self.devices.lock().unwrap().routed = target.copied();
        std::future::ready(Ok(()))
    }
}

/// One lane of the stand-in devices.
struct Lane {
    id: u64,
    audio: FakeAudio,
    sink: Option<IoSink>,
    stop: Arc<AtomicBool>,
    started: bool,
}

impl AudioIo for Lane {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.sink = Some(sink.clone());
        self.stop = Arc::default();
        self.started = true;
        {
            let mut devices = self.audio.devices.lock().unwrap();
            devices.running += 1;
            devices.sinks.insert(self.id, sink.clone());
        }
        let (id, audio, stop) = (self.id, self.audio.clone(), Arc::clone(&self.stop));
        std::thread::spawn(move || {
            sink.send(IoEvent::Ready);
            let mut phase = 0.0f32;
            while !stop.load(Ordering::SeqCst) {
                let routed = audio.devices.lock().unwrap().routed == Some(id);
                let speaking =
                    routed && audio.tone.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
                let frame = (0..160)
                    .map(|_| {
                        phase = (phase + 2.0 * std::f32::consts::PI * 220.0 / 16_000.0) % std::f32::consts::TAU;
                        if speaking {
                            0.3 * phase.sin()
                        } else {
                            0.0
                        }
                    })
                    .collect();
                sink.send(IoEvent::Captured(frame));
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(())
    }

    fn play(&mut self, utterance: &str, chunk: usize, _samples: Vec<f32>, _sample_rate: u32) {
        if let Some(sink) = &self.sink {
            sink.send(IoEvent::ChunkStarted { utterance: utterance.into(), chunk });
            if !self.audio.hold.load(Ordering::SeqCst) {
                sink.send(IoEvent::ChunkPlayed { utterance: utterance.into(), chunk });
            }
        }
    }

    fn stop_playback(&mut self) {}

    fn stop(&mut self) {
        if !std::mem::replace(&mut self.started, false) {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        let mut devices = self.audio.devices.lock().unwrap();
        devices.sinks.remove(&self.id);
        devices.running -= 1;
        if devices.running == 0 {
            devices.closed += 1;
        }
    }
}

impl Drop for Lane {
    fn drop(&mut self) {
        self.stop();
        self.audio.devices.lock().unwrap().attached -= 1;
    }
}

/// What reached the page, in order.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Value>>>);

impl Seen {
    fn page(&self) -> Page {
        let seen = Arc::clone(&self.0);
        Arc::new(move |event| seen.lock().unwrap().push(event))
    }

    /// The page, which takes `delay` over each idle state of `key`'s.
    fn slow_idle(&self, key: &'static str, delay: Duration) -> Page {
        let seen = Arc::clone(&self.0);
        Arc::new(move |event: Value| {
            let idle = event["key"] == key && event["type"] == "state" && event["data"]["listening"] == "idle";
            if idle {
                std::thread::sleep(delay);
            }
            seen.lock().unwrap().push(event);
        })
    }

    fn of(&self, key: &str) -> Vec<Value> {
        self.0.lock().unwrap().iter().filter(|event| event["key"] == key).cloned().collect()
    }

    /// How many of `key`'s events are of `kind` and, for a turn, in `phase`.
    fn count(&self, key: &str, kind: &str, phase: Option<&str>) -> usize {
        self.of(key)
            .iter()
            .filter(|event| event["type"] == kind && phase.is_none_or(|phase| event["data"]["phase"] == phase))
            .count()
    }

    fn last_listening(&self, key: &str) -> Option<String> {
        self.of(key)
            .iter()
            .rev()
            .find(|event| event["type"] == "state")
            .and_then(|event| event["data"]["listening"].as_str().map(str::to_owned))
    }

    async fn until(&self, what: &str, done: impl Fn(&Seen) -> bool) {
        let waited = tokio::time::timeout(Duration::from_secs(30), async {
            while !done(self) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        waited.await.unwrap_or_else(|_| panic!("in time: {what}"));
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap()
}

fn host() -> (VoiceState<FakeAudio>, FakeAudio) {
    let audio = FakeAudio::default();
    (VoiceState::new(audio.clone(), NativeDevices::default()), audio)
}

/// The call of `key`'s lane, as its first settings make it: a transcriber that answers "words", a speaker.
async fn configure(state: &VoiceState<FakeAudio>, key: &str, page: Page) {
    configure_with(state, key, page, config("silence", "voice")).await;
}

async fn configure_with(state: &VoiceState<FakeAudio>, key: &str, page: Page, config: VoiceConfig) {
    let (started, _) = tokio::sync::mpsc::unbounded_channel();
    let models = vec![Held::new("words", false, &started), Held::new("voice", false, &started)];
    let models = EngineModels::with(Box::new(FakeLoader::new(models, false)), chosen("words", "voice", None));
    let lane = state.lane(key).await.unwrap();
    let mut call = lane.call.lock().await;
    lane.install(&mut call, key, Arc::new(models), config, page).unwrap();
}

/// How many of `key`'s saying steps are of `step`.
fn say_steps(seen: &Seen, key: &str, step: &str) -> usize {
    seen.of(key).iter().filter(|event| event["type"] == "say" && event["data"]["event"]["type"] == step).count()
}

/// Two started lanes on one microphone: each hears only while routed to (the other gets silence, and None silences
/// both), a device failure on one lane is that call's error alone, disposing one releases its lane while the other
/// goes on hearing, and the last lane closes the devices.
#[test]
fn two_lanes_hear_only_their_route_and_the_last_one_closes_the_devices() {
    runtime().block_on(async {
        let (state, audio) = host();
        let seen = Seen::default();
        for key in ["a", "b"] {
            state.create_call(key.into()).await.unwrap();
            configure(&state, key, seen.page()).await;
            within(state.start(key, false)).await.unwrap();
        }
        assert_eq!((audio.count(|d| d.attached), audio.count(|d| d.running)), (2, 2));

        state.route_input(Some("a".into())).await.unwrap();
        audio.speak(TURN_FRAMES);
        seen.until("A's turn", |seen| seen.count("a", "turn", Some("finished")) == 1).await;
        assert_eq!(seen.count("b", "turn", None), 0, "B heard silence");

        state.route_input(Some("b".into())).await.unwrap();
        audio.speak(TURN_FRAMES);
        seen.until("B's turn", |seen| seen.count("b", "turn", Some("finished")) == 1).await;
        assert_eq!(seen.count("a", "turn", Some("started")), 1, "A heard silence");

        state.route_input(None).await.unwrap();
        audio.speak(TURN_FRAMES);
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert_eq!(
            (seen.count("a", "turn", Some("started")), seen.count("b", "turn", Some("started"))),
            (1, 1),
            "no lane hears while none is routed"
        );
        audio.speak(0);

        audio.fail(1);
        seen.until("A's error", |seen| seen.count("a", "error", None) == 1).await;
        assert_eq!(seen.count("b", "error", None), 0, "B's lane did not fail");

        within(state.dispose_call("a")).await.unwrap();
        audio.until("A's lane released", |d| d.attached == 1).await;
        state.route_input(Some("b".into())).await.unwrap();
        audio.speak(TURN_FRAMES);
        seen.until("B's next turn", |seen| seen.count("b", "turn", Some("finished")) == 2).await;
        assert_eq!(audio.count(|d| d.closed), 0, "B keeps the devices open");

        within(state.dispose_call("b")).await.unwrap();
        audio.until("every lane released", |d| d.attached == 0).await;
        assert_eq!((audio.count(|d| d.running), audio.count(|d| d.closed)), (0, 1), "the last lane closed the devices");
    });
}

/// A mute asked before the settings is kept: the call starts muted, though routed and spoken to, while another call
/// is not; a cancel before the settings is harmless.
#[test]
fn a_mute_before_the_settings_holds_once_the_call_starts() {
    runtime().block_on(async {
        let (state, audio) = host();
        let seen = Seen::default();
        state.create_call("a".into()).await.unwrap();
        state.create_call("b".into()).await.unwrap();
        state.mute("a", true).await.unwrap();
        state.cancel_input("a").await.unwrap();
        for key in ["a", "b"] {
            configure(&state, key, seen.page()).await;
            within(state.start(key, false)).await.unwrap();
        }
        state.route_input(Some("a".into())).await.unwrap();
        audio.speak(TURN_FRAMES);
        seen.until("A's tone heard out", |_| audio.tone.load(Ordering::SeqCst) == 0).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(seen.last_listening("a").as_deref(), Some("muted"));
        assert_eq!(seen.count("a", "turn", None), 0, "a muted call hears silence");

        state.route_input(Some("b".into())).await.unwrap();
        audio.speak(TURN_FRAMES);
        seen.until("B's turn", |seen| seen.count("b", "turn", Some("finished")) == 1).await;
    });
}

/// A start right after a stop, while the stop's idle state is still on its way to the page, listens again instead of
/// taking the old listening for its own; the stop answers once that idle state has reached the page. A second start
/// keeps the first's take, which reaches the page before the stop answers.
#[test]
fn a_start_after_a_stop_listens_again_and_a_second_start_keeps_the_take() {
    runtime().block_on(async {
        let (state, _audio) = host();
        let seen = Seen::default();
        state.create_call("a".into()).await.unwrap();
        configure(&state, "a", seen.slow_idle("a", Duration::from_millis(300))).await;

        within(state.start("a", true)).await.unwrap();
        within(state.start("a", false)).await.unwrap();
        assert!(within(state.stop("a")).await.unwrap());
        assert_eq!(seen.count("a", "take", None), 1, "the first start's take, before the stop answered");
        assert_eq!(seen.last_listening("a").as_deref(), Some("idle"));

        within(state.start("a", false)).await.unwrap();
        let stopping = state.stop("a");
        tokio::pin!(stopping);
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut stopping).await.is_err(), "the idle is slow");
        let (stopped, started) = tokio::join!(within(stopping), within(state.start("a", false)));
        assert!(stopped.unwrap());
        started.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_ne!(seen.last_listening("a").as_deref(), Some("idle"), "the second start listens");
        assert_eq!(seen.count("a", "take", None), 1, "no take was asked since");
    });
}

/// Disposing hands the page each call's own ends before it resolves: a saying heard in part ends as the call says
/// (never a made-up `not-played`), an open turn is cancelled, the take is handed over; nothing comes after.
#[test]
fn dispose_hands_over_the_says_end_the_open_turn_and_the_take_then_nothing() {
    runtime().block_on(async {
        let (state, audio) = host();
        let seen = Seen::default();
        audio.hold.store(true, Ordering::SeqCst);
        for key in ["a", "b"] {
            state.create_call(key.into()).await.unwrap();
            configure(&state, key, seen.page()).await;
        }
        within(state.start("a", false)).await.unwrap();
        state.say("a", "say-1".into(), "Hello there.".into(), None, seen.page()).await.unwrap();
        let steps = |seen: &Seen, step: &str| {
            seen.of("a").iter().filter(|event| event["type"] == "say" && event["data"]["event"]["type"] == step).count()
        };
        seen.until("A plays", |seen| steps(seen, "playing") == 1).await;

        within(state.start("b", true)).await.unwrap();
        state.route_input(Some("b".into())).await.unwrap();
        audio.speak(100_000);
        seen.until("B's open turn", |seen| seen.count("b", "turn", Some("started")) == 1).await;

        within(state.dispose_call("a")).await.unwrap();
        assert_eq!(steps(&seen, "done"), 1, "the saying's own end, before dispose resolved");
        within(state.dispose_call("b")).await.unwrap();
        assert_eq!(seen.count("b", "turn", Some("cancelled")), 1, "the open turn ended");
        assert_eq!(seen.count("b", "take", None), 1, "the take was handed over");

        let (a, b) = (seen.of("a").len(), seen.of("b").len());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!((seen.of("a").len(), seen.of("b").len()), (a, b), "nothing after dispose");
    });
}

/// endInput closes the open turn now, long before silence would (calm patience waits 3.5 s, then the 1.5 s merge
/// window), without muting; with no settings yet or no turn open it resolves at once, and a saying under way goes on
/// playing.
#[test]
fn end_input_closes_the_open_turn_now_and_leaves_the_playback_alone() {
    runtime().block_on(async {
        let (state, audio) = host();
        let seen = Seen::default();
        audio.hold.store(true, Ordering::SeqCst);
        for key in ["a", "b"] {
            state.create_call(key.into()).await.unwrap();
        }
        within(state.end_input("a")).await.unwrap();
        let calm: VoiceConfig = serde_json::from_value(serde_json::json!({
            "language": "en", "voice": "voice", "speed": 1.0, "end_of_turn": "silence", "patience": "calm",
        }))
        .unwrap();
        configure_with(&state, "a", seen.page(), calm).await;
        configure(&state, "b", seen.page()).await;
        for key in ["a", "b"] {
            within(state.start(key, false)).await.unwrap();
        }

        state.say("b", "say-1".into(), "Hello there.".into(), None, seen.page()).await.unwrap();
        seen.until("B plays", |seen| say_steps(seen, "b", "playing") == 1).await;
        within(state.end_input("b")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(say_steps(&seen, "b", "done"), 0, "the playback goes on");

        state.route_input(Some("a".into())).await.unwrap();
        audio.speak(100_000);
        seen.until("A's open turn", |seen| seen.count("a", "turn", Some("started")) == 1).await;
        audio.speak(0);
        let asked = std::time::Instant::now();
        within(state.end_input("a")).await.unwrap();
        seen.until("A's turn told", |seen| seen.count("a", "turn", Some("finished")) == 1).await;
        assert!(asked.elapsed() < Duration::from_millis(3_500), "closed by endInput, not by the calm patience");
        assert_eq!(seen.count("a", "turn", Some("cancelled")), 0);
        assert_ne!(seen.last_listening("a").as_deref(), Some("muted"), "endInput does not mute");
        assert_eq!(say_steps(&seen, "b", "done"), 0, "B's playback still goes on");
    });
}
