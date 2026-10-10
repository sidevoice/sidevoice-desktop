//! The adapter on runtimes of its own: its native work leaves the async workers free, and a whole call through it, on
//! the engine's real models, hears recorded speech and plays a reply. The real call downloads its models: it is
//! ignored unless asked for (`--ignored`), which the macOS CI job does.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use sidevoice_desktop_core::voice::{choose, Candidate, VoiceSettings};
use sidevoice_desktop_engine::sidevoice_engine::NoCredentials;
use sidevoice_desktop_engine::NativeEngines;
use sidevoice_voice::{
    AudioIo, Events, IoEvent, IoSink, SayEvent, SayOptions, SayOutcome, TurnEvent, VoiceCall, VoiceConfig, VoiceEvent,
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
    let catalogue: Vec<Candidate> = engines.models().unwrap().iter().map(crate::voice::candidate).collect();
    let settings: VoiceSettings = serde_json::from_value(json!({
        "stt": {"model": "whisper-tiny", "language": "en"},
        "tts": {"model": "kokoro-82m-v1.0"},
        "patience": "fast",
    }))
    .unwrap();
    let choice = choose(&settings, &catalogue).unwrap();
    assert_eq!(choice.stt.build, "whisper-tiny/whisper-cpp-q5_1", "Whisper on whisper.cpp");
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
