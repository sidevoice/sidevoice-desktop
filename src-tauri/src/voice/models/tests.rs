//! The adapter on runtimes of its own: its native work leaves the async workers free, and a whole call through it, on
//! the engine's real models, hears recorded speech and plays a reply. The real call downloads its models: it is
//! ignored unless asked for (`--ignored`), which the macOS CI job does.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use sidevoice_desktop_core::voice::{choose, CatalogCandidates, ModelChoice, VoiceSettings};
use sidevoice_desktop_engine::sidevoice_engine::NoCredentials;
use sidevoice_desktop_engine::NativeEngines;
use sidevoice_voice::{
    AudioIo, Events, IoEvent, IoSink, Listening, PlaybackState, SayEvent, SayOptions, SayOutcome, TurnEvent, VoiceCall,
    VoiceConfig, VoiceEvent,
};

use super::*;

#[test]
fn native_work_leaves_the_async_workers_free() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    runtime.block_on(async {
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = {
            let ticks = Arc::clone(&ticks);
            tokio::spawn(async move {
                loop {
                    ticks.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
        };
        // Half a second of native work, from a task on the only worker: the ticker on that worker keeps ticking.
        let work = tokio::spawn(blocking(async {
            std::thread::sleep(Duration::from_millis(500));
            Ok::<_, Error>(7)
        }));
        let before = ticks.load(Ordering::SeqCst);
        assert_eq!(work.await.unwrap(), Ok(7));
        let during = ticks.load(Ordering::SeqCst) - before;
        assert!(during >= 20, "the worker ticked {during} times in 500 ms");
        ticker.abort();
    });
}

/// The LibriSpeech clip (test/fixtures/voice), as samples from -1 to 1.
fn speech() -> Vec<f32> {
    let wav: &[u8] = include_bytes!("../../../../test/fixtures/voice/librispeech_mr_quilter.wav");
    let mut at = 12;
    while &wav[at..at + 4] != b"data" {
        at += 8 + u32::from_le_bytes(wav[at + 4..at + 8].try_into().unwrap()) as usize;
    }
    wav[at + 8..].chunks_exact(2).map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0).collect()
}

/// A microphone that says the clip, then silence, in real time, and a speaker that plays at once what it is given.
struct Fixture {
    sink: Option<IoSink>,
    stop: Arc<AtomicBool>,
    played: Arc<Mutex<Vec<usize>>>,
}

impl AudioIo for Fixture {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.sink = Some(sink.clone());
        let stop = Arc::clone(&self.stop);
        std::thread::spawn(move || {
            sink.send(IoEvent::Ready);
            let heard = speech().into_iter().chain(std::iter::repeat_n(0.0, 16_000 * 4)).collect::<Vec<_>>();
            for frame in heard.chunks(160) {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                sink.send(IoEvent::Captured(frame.to_vec()));
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(())
    }

    fn play(&mut self, utterance: &str, chunk: usize, samples: Vec<f32>, _sample_rate: u32) {
        self.played.lock().unwrap().push(samples.len());
        if let Some(sink) = &self.sink {
            sink.send(IoEvent::ChunkStarted { utterance: utterance.into(), chunk });
            sink.send(IoEvent::ChunkPlayed { utterance: utterance.into(), chunk });
        }
    }

    fn stop_playback(&mut self) {}

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// The next event `want` keeps, within ten minutes (the first run downloads the models); an error fails the test.
async fn next<T>(events: &mut Events, want: impl Fn(VoiceEvent) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            match events.next().await.expect("the call is alive") {
                VoiceEvent::Error(error) => panic!("the call failed: {}", error.code),
                event => {
                    if let Some(found) = want(event) {
                        return found;
                    }
                }
            }
        }
    })
    .await
    .expect("in time")
}

#[test]
#[ignore = "downloads real models; the macOS CI job runs it"]
fn a_call_on_the_engines_real_models_hears_speech_and_plays_a_reply() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    let root = std::env::temp_dir().join(format!("sidevoice-real-call-{}", std::process::id()));
    let engines = NativeEngines::new(&root, NoCredentials, runtime.handle().clone()).unwrap();
    let views = engines.catalogs().unwrap();
    let catalogs: Vec<CatalogCandidates> = views.iter().map(crate::voice::candidates).collect();
    // Transcribing with a build chosen, as the web's settings name one ("How it runs"): the first that runs here.
    let build = views[0]
        .models
        .iter()
        .find(|m| m.id == "whisper-tiny")
        .and_then(|m| m.local.as_ref())
        .and_then(|local| local.builds.iter().find(|build| build.available).map(|build| build.id.clone()));
    let build = build.expect("a whisper-tiny build runs here");
    let settings: VoiceSettings = serde_json::from_value(json!({
        "stt": {"catalog": "local", "model": "whisper-tiny", "language": "en", "build": build},
        "tts": {"catalog": "local", "model": "kokoro-82m-v1.0"},
        "patience": "fast",
    }))
    .unwrap();
    let choice = choose(&settings, &catalogs).unwrap();
    assert_eq!(choice.stt, ModelChoice { build: Some(build), ..ModelChoice::local("whisper-tiny") });
    assert_eq!(choice.vad, ModelChoice::local("silero-vad"));
    let config: VoiceConfig = serde_json::from_value(choice.config.clone()).unwrap();
    let played = Arc::new(Mutex::new(Vec::new()));
    let io = Fixture { sink: None, stop: Arc::default(), played: Arc::clone(&played) };
    runtime.block_on(async {
        let models = Arc::new(EngineModels::new(engines.engine(), choice));
        let (call, mut events) = VoiceCall::new(models, Box::new(io), config);
        call.start();
        let said = next(&mut events, |event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { text, .. }) => Some(text),
            _ => None,
        })
        .await;
        assert!(said.to_lowercase().contains("quilter"), "heard {said:?}");

        let text = "The tests pass on every platform.";
        let mut saying = call.say(text, SayOptions { language: Some("en".into()) });
        let mut steps = Vec::new();
        while let Some(step) = tokio::time::timeout(Duration::from_secs(600), saying.next()).await.expect("in time") {
            steps.push(step);
        }
        assert_eq!(steps.last(), Some(&SayEvent::Done { outcome: SayOutcome::Heard }), "{steps:?}");
        assert!(
            steps.contains(&SayEvent::Progress { sounding: None, heard_chars: text.chars().count() }),
            "heard to its end: {steps:?}"
        );
        let samples: usize = played.lock().unwrap().iter().sum();
        assert!(samples > 12_000, "Kokoro spoke {samples} samples");
        call.stop();
    });
    drop(engines);
    let _ = std::fs::remove_dir_all(&root);
}

// ---- A model changed mid-call (gap #31): the call goes on, and work under way ends on its own model ----

/// A microphone that says a tone while the test asks it to and is quiet otherwise, in real time, and a speaker that
/// plays at once what it is given.
struct Mic {
    sink: Option<IoSink>,
    stop: Arc<AtomicBool>,
    /// Frames of tone left to say.
    tone: Arc<AtomicUsize>,
}

impl AudioIo for Mic {
    fn start(&mut self, sink: IoSink) -> Result<(), String> {
        self.sink = Some(sink.clone());
        let (stop, tone) = (Arc::clone(&self.stop), Arc::clone(&self.tone));
        std::thread::spawn(move || {
            sink.send(IoEvent::Ready);
            let mut phase = 0.0f32;
            while !stop.load(Ordering::SeqCst) {
                let speaking = tone.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok();
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
            sink.send(IoEvent::ChunkPlayed { utterance: utterance.into(), chunk });
        }
    }

    fn stop_playback(&mut self) {}

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A detector that hears speech in any 32 ms window louder than a whisper.
#[derive(Default)]
struct LoudVad {
    pending: Vec<f32>,
    taken: u64,
}

#[async_trait]
impl Vad for LoudVad {
    async fn accept(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, String> {
        self.pending.extend_from_slice(pcm);
        let mut frames = Vec::new();
        while self.pending.len() >= 512 {
            let window: Vec<f32> = self.pending.drain(..512).collect();
            self.taken += 512;
            let loud = window.iter().map(|s| s * s).sum::<f32>() / 512.0 > 0.001;
            frames.push(VadFrame { end: self.taken, speech: loud, probability: None });
        }
        Ok(frames)
    }

    async fn reset(&mut self) {
        self.pending.clear();
        self.taken = 0;
    }
}

/// A model that answers `name`, each call telling `started` it began and, held, waiting until it is released.
struct Held {
    name: &'static str,
    started: tokio::sync::mpsc::UnboundedSender<&'static str>,
    /// Open: its calls end at once.
    gate: tokio::sync::watch::Sender<bool>,
    calls: AtomicUsize,
}

impl Held {
    fn new(name: &'static str, held: bool, started: &tokio::sync::mpsc::UnboundedSender<&'static str>) -> Arc<Self> {
        let (gate, _) = tokio::sync::watch::channel(!held);
        Arc::new(Held { name, started: started.clone(), gate, calls: AtomicUsize::new(0) })
    }

    /// Every call held, and every one to come, ends.
    fn release(&self) {
        self.gate.send_replace(true);
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    async fn answer(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let _ = self.started.send(self.name);
        let mut open = self.gate.subscribe();
        let _ = open.wait_for(|open| *open).await;
    }
}

#[async_trait]
impl Transcriber for Held {
    async fn transcribe(&self, _pcm: Vec<f32>, _rate: u32, _language: Option<String>) -> Result<String, String> {
        self.answer().await;
        Ok(self.name.to_string())
    }
}

#[async_trait]
impl Speaker for Held {
    async fn speak(
        &self,
        _text: String,
        voice: Option<String>,
        _language: Option<String>,
        _speed: f32,
    ) -> Result<(Vec<f32>, u32), String> {
        self.answer().await;
        // A voice is only ever the one chosen with this model.
        assert_eq!(voice.as_deref(), Some(self.name), "the voice chosen with {}", self.name);
        Ok((vec![0.1; 4_800], 24_000))
    }
}

#[async_trait]
impl EndOfTurnModel for Held {
    async fn end_of_turn(&self, _pcm: Vec<f32>, _rate: u32) -> Result<f32, String> {
        self.answer().await;
        Ok(1.0)
    }
}

/// Models by id, as the engine would load them: `fail` cannot be loaded, and while `slow` is shut a load of `slow-…`
/// waits.
struct FakeLoader {
    models: Vec<Arc<Held>>,
    slow: tokio::sync::watch::Sender<bool>,
}

impl FakeLoader {
    async fn find(&self, chosen: &ModelChoice) -> Result<Arc<Held>, String> {
        if chosen.model == "fail" {
            return Err("model-load-failed".into());
        }
        if chosen.model.starts_with("slow-") {
            let _ = self.slow.subscribe().wait_for(|open| *open).await;
        }
        Ok(Arc::clone(self.models.iter().find(|m| m.name == chosen.model).expect("a model of the test")))
    }
}

#[async_trait]
impl Loader for Arc<FakeLoader> {
    async fn vad(&self, _chosen: &ModelChoice) -> Result<Box<dyn Vad>, String> {
        Ok(Box::new(LoudVad::default()))
    }
    async fn transcriber(&self, chosen: &ModelChoice) -> Result<Arc<dyn Transcriber>, String> {
        Ok(self.find(chosen).await?)
    }
    async fn speaker(&self, chosen: &ModelChoice) -> Result<Arc<dyn Speaker>, String> {
        Ok(self.find(chosen).await?)
    }
    async fn end_of_turn(&self, chosen: &ModelChoice) -> Result<Arc<dyn EndOfTurnModel>, String> {
        Ok(self.find(chosen).await?)
    }
}

/// The choice of `stt` and `tts` (and its voice, the same name), with `end_of_turn` when smart.
fn chosen(stt: &str, tts: &str, end_of_turn: Option<&str>) -> VoiceChoice {
    VoiceChoice {
        vad: ModelChoice::local("vad"),
        stt: ModelChoice::local(stt),
        tts: ModelChoice::local(tts),
        end_of_turn: end_of_turn.map(ModelChoice::local),
        config: json!({ "voice": tts }),
    }
}

fn config(end_of_turn: &str, voice: &str) -> VoiceConfig {
    serde_json::from_value(json!({
        "language": "en", "voice": voice, "speed": 1.0, "end_of_turn": end_of_turn, "patience": "fast",
    }))
    .unwrap()
}

const ALWAYS: &(dyn Fn() -> bool + Send + Sync) = &|| true;

/// A call on `models`, as the app runs one (the call's playback state told to the models), and what the test sees of
/// it. Every wait is bounded; an error, or the call going idle, fails the test: a model changed never stops the call.
struct Live {
    call: VoiceCall,
    events: Events,
    models: Arc<EngineModels>,
    tone: Arc<AtomicUsize>,
    began: tokio::sync::mpsc::UnboundedReceiver<&'static str>,
    /// Whether the call has listened: from then on it must never be idle.
    listened: bool,
}

impl Live {
    fn start(models: Arc<EngineModels>, began: tokio::sync::mpsc::UnboundedReceiver<&'static str>) -> Self {
        let tone = Arc::new(AtomicUsize::new(0));
        let mic = Mic { sink: None, stop: Arc::default(), tone: Arc::clone(&tone) };
        let (call, events) =
            VoiceCall::new(Arc::clone(&models) as Arc<dyn VoiceModels>, Box::new(mic), config("silence", "af_old"));
        call.start();
        Live { call, events, models, tone, began, listened: false }
    }

    async fn next<T>(&mut self, want: impl Fn(&VoiceEvent) -> Option<T>) -> T {
        let (events, models, listened) = (&mut self.events, &self.models, &mut self.listened);
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let event = events.next().await.expect("the call is alive");
                match &event {
                    VoiceEvent::Error(error) => panic!("the call failed: {}", error.code),
                    VoiceEvent::State(state) => {
                        if state.listening != Listening::Idle {
                            *listened = true;
                        }
                        assert!(!*listened || state.listening != Listening::Idle, "the call stopped");
                        models.playback(state.playback == PlaybackState::Idle);
                    }
                    _ => {}
                }
                if let Some(found) = want(&event) {
                    return found;
                }
            }
        })
        .await
        .expect("in time")
    }

    /// A turn of the person's, and its words.
    async fn turn(&mut self) -> String {
        self.tone.store(80, Ordering::SeqCst);
        self.next(|event| match event {
            VoiceEvent::Turn(TurnEvent::Finished { text, .. }) => Some(text.clone()),
            _ => None,
        })
        .await
    }

    /// The model the next call to one starts on.
    async fn began(&mut self) -> &'static str {
        tokio::time::timeout(Duration::from_secs(30), self.began.recv()).await.expect("in time").expect("a model")
    }

    /// Says `text` to its end, then waits for the speaker to be idle; its outcome.
    async fn say(&mut self, text: &str) -> SayOutcome {
        let mut saying = self.call.say(text, SayOptions::default());
        let mut outcome = None;
        while let Some(step) = tokio::time::timeout(Duration::from_secs(30), saying.next()).await.expect("in time") {
            if let SayEvent::Done { outcome: done } = step {
                outcome = Some(done);
            }
        }
        self.idle().await;
        outcome.expect("an outcome")
    }

    async fn idle(&mut self) {
        self.next(|event| match event {
            VoiceEvent::State(state) if state.playback == PlaybackState::Idle => Some(()),
            _ => None,
        })
        .await;
    }
}

/// Three sentences: a reply of several chunks.
const LONG: &str = "This first sentence is long enough to be a chunk of its own when it is spoken. \
    The second one is just as long, so that the reply goes on after the first. \
    And a third, to be sure the reply has several chunks to speak in turn.";

/// Through the app's own switch: a turn being transcribed when the transcriber changes ends on the one it started
/// with, and the next turn is the new one's; a reply being spoken when the speaker changes is spoken to its end by the
/// one it started with, in its voice, and the next reply is the new one's. The call never stops.
#[test]
fn a_model_switched_mid_turn_and_mid_reply_does_not_stop_the_call() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    runtime.block_on(async {
        let (started, began) = tokio::sync::mpsc::unbounded_channel();
        let old_stt = Held::new("old", true, &started);
        let new_stt = Held::new("new", false, &started);
        let old_tts = Held::new("af_old", true, &started);
        let new_tts = Held::new("af_new", false, &started);
        let loader = Arc::new(FakeLoader {
            models: vec![old_stt.clone(), new_stt.clone(), old_tts.clone(), new_tts.clone()],
            slow: tokio::sync::watch::channel(true).0,
        });
        let models = Arc::new(EngineModels::with(Box::new(loader), chosen("old", "af_old", None)));
        let mut live = Live::start(Arc::clone(&models), began);

        // A turn: its transcription is held while the transcriber switches.
        live.tone.store(80, Ordering::SeqCst);
        assert_eq!(live.began().await, "old");
        let switched = models.switch(chosen("new", "af_old", None), ALWAYS).await.unwrap();
        assert!(switched.is_some());
        drop(switched);
        old_stt.release();
        let first = live
            .next(|event| match event {
                VoiceEvent::Turn(TurnEvent::Finished { text, .. }) => Some(text.clone()),
                _ => None,
            })
            .await;
        assert!(first.contains("old"), "the turn under way ended on its own model: {first:?}");
        let second = live.turn().await;
        assert!(second.contains("new") && !second.contains("old"), "the next turn is the new model's: {second:?}");

        // A reply of several chunks: its first is held while the speaker switches; the whole reply is the old one's.
        let mut saying = live.call.say(LONG, SayOptions::default());
        while live.began().await != "af_old" {}
        drop(models.switch(chosen("new", "af_new", None), ALWAYS).await.unwrap().expect("switched"));
        live.call.set_config(config("silence", "af_new"));
        old_tts.release();
        let mut outcome = None;
        while let Some(step) = tokio::time::timeout(Duration::from_secs(30), saying.next()).await.expect("in time") {
            if let SayEvent::Done { outcome: done } = step {
                outcome = Some(done);
            }
        }
        assert_eq!(outcome, Some(SayOutcome::Heard), "the reply under way was heard");
        assert!(old_tts.calls() >= 2, "the reply had several chunks: {}", old_tts.calls());
        assert_eq!(new_tts.calls(), 0, "no chunk of it went to the new speaker");
        live.idle().await;
        assert_eq!(live.say("And now the new voice.").await, SayOutcome::Heard);
        assert!(new_tts.calls() >= 1, "the next reply is the new speaker's");

        // Still listening.
        live.turn().await;
        live.call.stop();
    });
}

/// A model that cannot be loaded changes nothing: the call goes on, on the model it had. Of two switches, the later
/// one wins even when the earlier one is still loading: the earlier one's models are dropped.
#[test]
fn a_refused_model_keeps_the_current_one_and_the_latest_switch_wins() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    runtime.block_on(async {
        let (started, began) = tokio::sync::mpsc::unbounded_channel();
        let old = Held::new("old", false, &started);
        let slow = Held::new("slow-one", false, &started);
        let last = Held::new("last", false, &started);
        let tts = Held::new("af_old", false, &started);
        let (slow_gate, _) = tokio::sync::watch::channel(false);
        let loader = Arc::new(FakeLoader {
            models: vec![old.clone(), slow.clone(), last.clone(), tts.clone()],
            slow: slow_gate,
        });
        let models = Arc::new(EngineModels::with(Box::new(Arc::clone(&loader)), chosen("old", "af_old", None)));
        let mut live = Live::start(Arc::clone(&models), began);
        assert!(live.turn().await.contains("old"));

        let refused = models.switch(chosen("fail", "af_old", None), ALWAYS).await.err();
        assert_eq!(refused.as_deref(), Some("model-load-failed"));
        assert!(live.turn().await.contains("old"), "the call goes on, on the model it had");

        // The earlier switch loads slowly; a later one is asked meanwhile, and wins.
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let first = {
            let (models, counter) = (Arc::clone(&models), Arc::clone(&counter));
            tokio::spawn(async move {
                let latest = move || counter.load(Ordering::SeqCst) == 1;
                models.switch(chosen("slow-one", "af_old", None), &latest).await.map(|switched| switched.is_some())
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        counter.store(2, Ordering::SeqCst);
        let second = {
            let (models, counter) = (Arc::clone(&models), Arc::clone(&counter));
            tokio::spawn(async move {
                let latest = move || counter.load(Ordering::SeqCst) == 2;
                models.switch(chosen("last", "af_old", None), &latest).await.map(|switched| switched.is_some())
            })
        };
        loader.slow.send_replace(true);
        assert_eq!(first.await.unwrap(), Ok(false), "overtaken: it changes nothing");
        assert_eq!(second.await.unwrap(), Ok(true));
        let heard = live.turn().await;
        assert!(heard.contains("last") && slow.calls() == 0, "the latest switch's model: {heard:?}");
        live.call.stop();
    });
}

/// A call on silence turns to smart-turn without stopping: the switch puts the end-of-turn model in its slot, then the
/// configuration asks for it, and the next pause asks it. Back to silence, the classifier stays in its slot, so a pause
/// already asked of it is answered.
#[test]
fn smart_turn_and_back_mid_call_does_not_stop_it() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    runtime.block_on(async {
        let (started, began) = tokio::sync::mpsc::unbounded_channel();
        let stt = Held::new("heard", false, &started);
        let tts = Held::new("af_old", false, &started);
        let ends = Held::new("ends", false, &started);
        let loader =
            Arc::new(FakeLoader { models: vec![stt, tts, ends.clone()], slow: tokio::sync::watch::channel(true).0 });
        let models = Arc::new(EngineModels::with(Box::new(loader), chosen("heard", "af_old", None)));
        let mut live = Live::start(Arc::clone(&models), began);
        live.turn().await;

        drop(models.switch(chosen("heard", "af_old", Some("ends")), ALWAYS).await.unwrap().expect("switched"));
        live.call.set_config(config("smart-turn", "af_old"));
        live.turn().await;
        assert!(ends.calls() > 0, "the pause asked the end-of-turn model");

        drop(models.switch(chosen("heard", "af_old", None), ALWAYS).await.unwrap().expect("switched"));
        live.call.set_config(config("silence", "af_old"));
        live.turn().await;
        live.call.stop();
    });
}
