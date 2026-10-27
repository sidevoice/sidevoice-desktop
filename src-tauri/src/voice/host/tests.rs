//! The native host's lanes, through the same methods the commands use, over a stand-in for the shared devices: one
//! microphone routed to one lane (a tone while the test asks for one, zeros to every other lane) and a speaker that
//! plays at once, or holds what it is given. Real `VoiceCall`s on the adapter's halves with fake loaders; the page is
//! a list of what reached it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use sidevoice_desktop_core::voice::{InputSettings, OutputSettings, VoiceChoice};
use sidevoice_voice::{AudioIo, IoEvent, IoSink, NativeDevices};

use super::super::models::tests::{chosen, loader, FakeLoader, TURN_FRAMES};
use super::super::models::Halves;
use super::{Applied, LaneAudio, Page, VoiceSettings, VoiceState};

/// A minute of the host's idle minutes, short.
const MINUTE: Duration = Duration::from_millis(100);

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
    /// How many starts asked for the microphone, and how many for none.
    with_microphone: usize,
    without_microphone: usize,
    /// What `set_input` was asked, in order.
    set_input: Vec<bool>,
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
        let audio = self.clone();
        let waited = tokio::time::timeout(Duration::from_secs(30), async move {
            while !done(&audio.devices.lock().unwrap()) {
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
        let lane = Lane {
            id,
            audio: self.clone(),
            sink: None,
            stop: Arc::default(),
            started: false,
            microphone: Arc::default(),
        };
        (Box::new(lane), id)
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
    /// Whether the microphone is open: it gives tone (to the routed lane) or silence only then.
    microphone: Arc<AtomicBool>,
}

impl AudioIo for Lane {
    fn start(&mut self, sink: IoSink, input: bool) -> Result<(), String> {
        self.sink = Some(sink.clone());
        self.stop = Arc::default();
        self.started = true;
        self.microphone.store(input, Ordering::SeqCst);
        {
            let mut devices = self.audio.devices.lock().unwrap();
            devices.running += 1;
            devices.sinks.insert(self.id, sink.clone());
            if input {
                devices.with_microphone += 1;
            } else {
                devices.without_microphone += 1;
            }
        }
        let (id, audio, stop, microphone) =
            (self.id, self.audio.clone(), Arc::clone(&self.stop), Arc::clone(&self.microphone));
        std::thread::spawn(move || {
            sink.send(IoEvent::Ready);
            let mut phase = 0.0f32;
            while !stop.load(Ordering::SeqCst) {
                if microphone.load(Ordering::SeqCst) {
                    let routed = audio.devices.lock().unwrap().routed == Some(id);
                    let speaking = routed
                        && audio.tone.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
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
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(())
    }

    fn set_input(&mut self, input: bool) {
        self.audio.devices.lock().unwrap().set_input.push(input);
        self.microphone.store(input, Ordering::SeqCst);
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

/// `work`, within thirty seconds.
async fn within<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), work).await.expect("in time")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap()
}

fn host() -> (VoiceState<FakeAudio>, FakeAudio) {
    let audio = FakeAudio::default();
    let mut state = VoiceState::new(audio.clone(), NativeDevices::default());
    state.minute = MINUTE;
    (state, audio)
}

/// The settings a call of `choice` came from: what the page asked, kept with the call (its halves only as far as the
/// host reads them, and `idle` minutes).
fn settings(choice: &VoiceChoice, idle: u32) -> VoiceSettings {
    let stt = serde_json::from_value(serde_json::json!({"catalog": "local", "model": "m"})).unwrap();
    let tts = serde_json::from_value(serde_json::json!({"catalog": "local", "model": "t"})).unwrap();
    VoiceSettings {
        input: choice.input.as_ref().map(|_| InputSettings {
            stt,
            end_of_turn: None,
            patience: None,
            listening_bar: None,
            barge_in_min_speech: None,
            barge_in_window: None,
            vad_confidence: None,
            speech_start_ms: None,
            pre_roll_ms: None,
        }),
        output: choice.output.as_ref().map(|_| OutputSettings {
            tts,
            voice: None,
            speed: None,
            fade_ms: None,
            language: None,
        }),
        audio_grace_ms: None,
        idle_unload_minutes: Some(idle),
    }
}

fn applied(fake: &Arc<FakeLoader>, choice: &VoiceChoice, previous: Option<&Applied>, idle: u32) -> Applied {
    Applied {
        settings: settings(choice, idle),
        halves: Halves::new(&loader(fake), choice, previous.map(|previous| &previous.halves)),
    }
}

/// The call of `key`'s lane, started with both halves: a transcriber that answers "words", a speaker.
async fn configure(state: &VoiceState<FakeAudio>, fake: &Arc<FakeLoader>, key: &str, page: Page) {
    configure_with(state, fake, key, page, chosen(Some("words"), Some("voice"), None), false).await;
}

async fn configure_with(
    state: &VoiceState<FakeAudio>,
    fake: &Arc<FakeLoader>,
    key: &str,
    page: Page,
    choice: VoiceChoice,
    take: bool,
) {
    state.open(key, applied(fake, &choice, None, 0), take, page).await.unwrap();
}

/// A started call, working: the lane made, the call opened on both halves, ready.
async fn started(state: &VoiceState<FakeAudio>, fake: &Arc<FakeLoader>, key: &str, page: Page, take: bool) {
    state.create_call(key.into()).await.unwrap();
    configure_with(state, fake, key, page, chosen(Some("words"), Some("voice"), None), take).await;
    within(state.ready(key)).await.unwrap();
}

fn fake() -> Arc<FakeLoader> {
    FakeLoader::new(&["words", "voice", "other", "ends"])
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
        let (seen, fake) = (Seen::default(), fake());
        for key in ["a", "b"] {
            started(&state, &fake, key, seen.page(), false).await;
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

/// A mute asked before the call is kept: the call starts muted, though routed and spoken to, while another call is
/// not; a cancel before the call is harmless.
#[test]
fn a_mute_before_the_call_holds_once_it_starts() {
    runtime().block_on(async {
        let (state, audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        state.create_call("a".into()).await.unwrap();
        state.create_call("b".into()).await.unwrap();
        state.mute("a", true).await.unwrap();
        state.cancel_input("a").await.unwrap();
        within(state.end_input("a")).await.unwrap();
        for key in ["a", "b"] {
            configure(&state, &fake, key, seen.page()).await;
            within(state.ready(key)).await.unwrap();
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

/// A call asked to stop before it works says so: `ready` rejects `stopped`, and still does when it is asked after.
/// The stop answers only once the idle state that answers it has reached the page, the take before it.
#[test]
fn ready_rejects_stopped_and_a_stop_answers_once_the_page_has_the_idle_state_and_the_take() {
    runtime().block_on(async {
        let (state, _audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        state.create_call("a".into()).await.unwrap();
        configure_with(
            &state,
            &fake,
            "a",
            seen.slow_idle("a", Duration::from_millis(300)),
            chosen(Some("words"), Some("voice"), None),
            true,
        )
        .await;
        within(state.ready("a")).await.unwrap();
        let stopping = state.stop("a");
        tokio::pin!(stopping);
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut stopping).await.is_err(), "the idle is slow");
        assert!(within(stopping).await.unwrap());
        assert_eq!(seen.count("a", "take", None), 1, "the take, before the stop answered");
        assert_eq!(seen.last_listening("a").as_deref(), Some("idle"));
        within(state.dispose_call("a")).await.unwrap();

        // Stopped while its models were still loading, it never works.
        let slow = FakeLoader::new(&["words", "voice", "slow"]);
        state.create_call("b".into()).await.unwrap();
        configure_with(&state, &slow, "b", seen.page(), chosen(Some("slow"), Some("voice"), None), false).await;
        let (ready, stopped) = tokio::join!(within(state.ready("b")), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let ran = within(state.stop("b")).await;
            slow.open_gate();
            ran
        });
        assert!(stopped.unwrap());
        let refused = ready.unwrap_err();
        assert_eq!(refused.params["code"], "stopped");
        assert_eq!(within(state.ready("b")).await.unwrap_err().params["code"], "stopped", "kept for a later ask");
    });
}

/// A call that fails before it works, here on a model that cannot load, rejects `ready` with its own code, however
/// late `ready` is asked.
#[test]
fn ready_rejects_with_the_code_of_what_failed() {
    runtime().block_on(async {
        let (state, _audio) = host();
        let (seen, fake) = (Seen::default(), FakeLoader::new(&["words", "voice", "fail"]));
        state.create_call("a".into()).await.unwrap();
        configure_with(&state, &fake, "a", seen.page(), chosen(Some("fail"), Some("voice"), None), false).await;
        let early = within(state.ready("a")).await.unwrap_err();
        assert_eq!(early.params["code"], "model-load-failed");
        seen.until("the error told", |seen| seen.count("a", "error", None) == 1).await;
        let late = within(state.ready("a")).await.unwrap_err();
        assert_eq!(late.params["code"], "model-load-failed");
    });
}

/// A call with an output half only asks for no microphone and loads no detector, transcriber or end-of-turn model: it
/// works (`off`, nothing is heard) and it speaks.
#[test]
fn a_call_without_an_input_half_opens_no_microphone_and_loads_only_its_speaker() {
    runtime().block_on(async {
        let (state, audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        state.create_call("a".into()).await.unwrap();
        configure_with(&state, &fake, "a", seen.page(), chosen(None, Some("voice"), None), false).await;
        within(state.ready("a")).await.unwrap();
        assert_eq!(seen.last_listening("a").as_deref(), Some("off"));
        assert_eq!((audio.count(|d| d.with_microphone), audio.count(|d| d.without_microphone)), (0, 1));
        state.say("a", "say-1".into(), "Hello there.".into(), None, seen.page()).await.unwrap();
        seen.until("it speaks", |seen| say_steps(seen, "a", "done") == 1).await;
        assert_eq!(fake.loads.lock().unwrap().clone(), ["tts:voice"]);
        within(state.dispose_call("a")).await.unwrap();
    });
}

/// A call with an input half only loads no speaker: what it is asked to say is not played, and it still hears.
#[test]
fn a_call_without_an_output_half_loads_no_speaker_and_says_nothing() {
    runtime().block_on(async {
        let (state, audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        state.create_call("a".into()).await.unwrap();
        configure_with(&state, &fake, "a", seen.page(), chosen(Some("words"), None, None), false).await;
        within(state.ready("a")).await.unwrap();
        state.say("a", "say-1".into(), "Hello there.".into(), None, seen.page()).await.unwrap();
        seen.until("it is not played", |seen| say_steps(seen, "a", "done") == 1).await;
        let done = seen.of("a").into_iter().find(|event| event["type"] == "say").unwrap();
        assert_eq!(done["data"]["event"]["outcome"]["status"], "not-played");
        assert_eq!(fake.loaded("tts"), 0);

        state.route_input(Some("a".into())).await.unwrap();
        audio.speak(TURN_FRAMES);
        seen.until("it hears", |seen| seen.count("a", "turn", Some("finished")) == 1).await;
        within(state.dispose_call("a")).await.unwrap();
    });
}

/// An update changes a running call: with the same halves it loads nothing, with another transcriber it loads that
/// one, and a half added later (a microphone that appears) opens the microphone and loads what the half uses, without
/// restarting the call.
#[test]
fn an_update_keeps_what_the_halves_loaded_and_adds_a_half_without_restarting() {
    runtime().block_on(async {
        let (state, audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        state.create_call("a".into()).await.unwrap();
        configure_with(&state, &fake, "a", seen.page(), chosen(None, Some("voice"), None), false).await;
        within(state.ready("a")).await.unwrap();
        assert_eq!(fake.loads.lock().unwrap().clone(), ["tts:voice"]);

        // The same halves, another voice: nothing is loaded again.
        let now = state.applied("a").await.unwrap();
        let mut voiced = chosen(None, Some("voice"), None);
        voiced.output.as_mut().unwrap().config = serde_json::json!({"voice": "af_heart", "speed": 0.9});
        state.update("a", applied(&fake, &voiced, Some(&now), 0)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(fake.loaded("tts"), 1);

        // A microphone appears: the input half is added, loaded beside the running call, and the microphone opens.
        let now = state.applied("a").await.unwrap();
        let both = chosen(Some("words"), Some("voice"), None);
        state.update("a", applied(&fake, &both, Some(&now), 0)).await.unwrap();
        audio.until("the microphone opens", |d| d.set_input.contains(&true)).await;
        assert_eq!((fake.loaded("stt"), fake.loaded("vad"), fake.loaded("tts")), (1, 1, 1));
        state.route_input(Some("a".into())).await.unwrap();
        audio.speak(TURN_FRAMES);
        seen.until("it hears", |seen| seen.count("a", "turn", Some("finished")) == 1).await;
        assert_ne!(seen.last_listening("a").as_deref(), Some("idle"), "the call never stopped");
        assert_eq!(audio.count(|d| d.running), 1);

        // An update of a call not started is refused; the half removed closes the microphone.
        let now = state.applied("a").await.unwrap();
        state.update("a", applied(&fake, &chosen(None, Some("voice"), None), Some(&now), 0)).await.unwrap();
        audio.until("the microphone closes", |d| d.set_input.last() == Some(&false)).await;
        state.create_call("b".into()).await.unwrap();
        assert_eq!(state.applied("b").await.err().map(|r| r.params["code"].clone()), Some("not-started".into()));
        within(state.dispose_call("a")).await.unwrap();
    });
}

/// A stopped call keeps what it loaded for its idle minutes: a call that asks for the same models meanwhile finds
/// them, and they are let go after; with no idle minutes they go at once.
#[test]
fn a_disposed_call_keeps_its_models_for_its_idle_minutes() {
    runtime().block_on(async {
        let (state, _audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        let both = chosen(Some("words"), Some("voice"), None);

        for (minutes, key) in [(3, "a"), (0, "b")] {
            state.create_call(key.into()).await.unwrap();
            state.open(key, applied(&fake, &both, None, minutes), false, seen.page()).await.unwrap();
            within(state.ready(key)).await.unwrap();
            assert_eq!(fake.holders("words"), 1, "the running call holds its transcriber");
            within(state.dispose_call(key)).await.unwrap();
            if minutes > 0 {
                tokio::time::sleep(Duration::from_millis(150)).await;
                assert_eq!(fake.holders("words"), 1, "kept after the stop, within the idle minutes");
                tokio::time::sleep(MINUTE * 4).await;
            } else {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            assert_eq!(fake.holders("words"), 0, "let go with {minutes} idle minutes");
        }
    });
}

/// A preload loads what the halves use, once, and keeps it for its idle minutes counted from the end of the load; a
/// slot that cannot load is its error, what loaded stays.
#[test]
fn a_preload_holds_what_it_loaded_for_the_idle_minutes() {
    runtime().block_on(async {
        let (state, _audio) = host();
        let fake = FakeLoader::new(&["words", "voice", "fail"]);
        let halves = |stt: &str| Halves::new(&loader(&fake), &chosen(Some(stt), Some("voice"), None), None);

        within(state.preload(halves("words"), 3)).await.unwrap();
        let mut loads = fake.loads.lock().unwrap().clone();
        loads.sort();
        assert_eq!(loads, ["stt:words", "tts:voice", "vad:vad"]);
        assert_eq!((fake.holders("words"), fake.holders("voice")), (1, 1));
        tokio::time::sleep(MINUTE * 5).await;
        assert_eq!((fake.holders("words"), fake.holders("voice")), (0, 0), "let go after the idle minutes");

        let refused = within(state.preload(halves("fail"), 3)).await.unwrap_err();
        assert_eq!(refused, "model-load-failed");
        assert_eq!(fake.holders("voice"), 1, "the speaker that loaded stays");
        tokio::time::sleep(MINUTE * 5).await;
        assert_eq!(fake.holders("voice"), 0);

        // With no idle minutes, it loads and lets go at once.
        within(state.preload(halves("words"), 0)).await.unwrap();
        assert_eq!(fake.holders("words"), 0);
    });
}

/// Disposing hands the page each call's own ends before it resolves: a saying heard in part ends as the call says
/// (never a made-up `not-played`), an open turn is cancelled, the take is handed over; nothing comes after.
#[test]
fn dispose_hands_over_the_says_end_the_open_turn_and_the_take_then_nothing() {
    runtime().block_on(async {
        let (state, audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        audio.hold.store(true, Ordering::SeqCst);
        for key in ["a", "b"] {
            state.create_call(key.into()).await.unwrap();
        }
        configure(&state, &fake, "a", seen.page()).await;
        within(state.ready("a")).await.unwrap();
        state.say("a", "say-1".into(), "Hello there.".into(), None, seen.page()).await.unwrap();
        seen.until("A plays", |seen| say_steps(seen, "a", "playing") == 1).await;

        configure_with(&state, &fake, "b", seen.page(), chosen(Some("words"), Some("voice"), None), true).await;
        within(state.ready("b")).await.unwrap();
        state.route_input(Some("b".into())).await.unwrap();
        audio.speak(100_000);
        seen.until("B's open turn", |seen| seen.count("b", "turn", Some("started")) == 1).await;

        within(state.dispose_call("a")).await.unwrap();
        assert_eq!(say_steps(&seen, "a", "done"), 1, "the saying's own end, before dispose resolved");
        within(state.dispose_call("b")).await.unwrap();
        assert_eq!(seen.count("b", "turn", Some("cancelled")), 1, "the open turn ended");
        assert_eq!(seen.count("b", "take", None), 1, "the take was handed over");

        let (a, b) = (seen.of("a").len(), seen.of("b").len());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!((seen.of("a").len(), seen.of("b").len()), (a, b), "nothing after dispose");
    });
}

/// endInput closes the open turn now, long before silence would (calm patience waits 3.5 s, then the 1.5 s merge
/// window), without muting; with no call yet or no turn open it resolves at once, and a saying under way goes on
/// playing.
#[test]
fn end_input_closes_the_open_turn_now_and_leaves_the_playback_alone() {
    runtime().block_on(async {
        let (state, audio) = host();
        let (seen, fake) = (Seen::default(), fake());
        audio.hold.store(true, Ordering::SeqCst);
        for key in ["a", "b"] {
            state.create_call(key.into()).await.unwrap();
        }
        within(state.end_input("a")).await.unwrap();
        let mut calm = chosen(Some("words"), Some("voice"), None);
        calm.input.as_mut().unwrap().config = serde_json::json!({
            "language": "en", "patience": "calm", "end_of_turn": {"kind": "silence"},
        });
        configure_with(&state, &fake, "a", seen.page(), calm, false).await;
        configure(&state, &fake, "b", seen.page()).await;
        for key in ["a", "b"] {
            within(state.ready(key)).await.unwrap();
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

/// Quitting lets go of every model, well before their idle minutes: a running call's, a stopped call's and a
/// preload's, and keeps nothing a preload loads after it. Driven as the app quits: from a thread outside the runtime's
/// context (`RunEvent::Exit`), through `block_on`.
#[test]
fn close_lets_go_of_every_model_before_its_idle_minutes() {
    let runtime = runtime();
    let (state, _audio) = host();
    let (seen, fake) = (Seen::default(), fake());
    let other = || Halves::new(&loader(&fake), &chosen(Some("other"), None, None), None);
    runtime.block_on(async {
        let both = chosen(Some("words"), Some("voice"), None);
        started(&state, &fake, "running", seen.page(), false).await;
        state.create_call("stopped".into()).await.unwrap();
        state.open("stopped", applied(&fake, &both, None, 30), false, seen.page()).await.unwrap();
        within(state.ready("stopped")).await.unwrap();
        within(state.dispose_call("stopped")).await.unwrap();
        within(state.preload(other(), 30)).await.unwrap();
        assert!(fake.holders("words") > 0 && fake.holders("other") > 0, "loaded before the quit");
    });
    let handle = runtime.handle().clone();
    let (held, state) = std::thread::spawn(move || {
        let held = handle.block_on(state.close(Duration::from_secs(5), Duration::from_millis(10)));
        (held, state)
    })
    .join()
    .unwrap();
    assert!(held.is_empty(), "nothing held: {held:?}");
    // A dropped call's own task lets go of its models at a time of its own, soon; the idle minutes are 3 s.
    let gone = |model: &str| {
        for _ in 0..50 {
            if fake.holders(model) == 0 {
                return true;
            }
            std::thread::sleep(MINUTE / 5);
        }
        false
    };
    for model in ["words", "voice", "other"] {
        assert!(gone(model), "{model} let go at the quit, before its idle minutes");
    }
    runtime.block_on(async { within(state.preload(other(), 30)).await.unwrap() });
    assert_eq!(fake.holders("other"), 0, "a preload after the quit keeps nothing");
}

/// A call on the machine's own microphone and speaker (no stand-in), as a hosted CI runner has them, which is often with
/// none: it is ready, or it fails by the code of what failed, within the time `ready` gives; it never hangs, panics or
/// takes the process down, and the lane it made is released either way. Which of the two depends on the machine.
#[test]
fn a_call_on_the_machines_own_devices_is_ready_or_fails_by_a_code_and_the_lane_is_released() {
    runtime().block_on(async {
        let state = VoiceState::default();
        let (seen, fake) = (Seen::default(), fake());
        state.create_call("a".into()).await.unwrap();
        state
            .open("a", applied(&fake, &chosen(Some("words"), Some("voice"), None), None, 0), false, seen.page())
            .await
            .unwrap();
        if let Err(refused) = within(state.ready("a")).await {
            let code = refused.params["code"].as_str().unwrap_or_default();
            assert!(!code.is_empty(), "a failed call says why: {refused:?}");
        }
        within(state.dispose_call("a")).await.unwrap();
        assert!(within(state.ready("a")).await.is_err(), "the lane is gone");
    });
}
