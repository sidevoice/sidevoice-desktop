//! The call's memory on the engine's real models (#145): the process's footprint, and what it holds on the GPU, at
//! each step of a long session through the host the app runs (`VoiceState`, the same `open`, `update`, `say`,
//! `dispose_call` and idle keep the commands use), over a stand-in for the devices that says a recorded sentence. Each
//! step prints a line `memory phase=NAME footprint=N ...` (core `memory`), which `cargo xtask probe voice-memory`
//! reads back and checks. Ignored unless asked for: it downloads the models and takes minutes.
//!
//! The host's idle minute is two seconds here, so "the models are let go after the call" is waited for, not assumed.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sidevoice_desktop_core::voice::{choose, CatalogCandidates, VoiceSettings};
use sidevoice_desktop_engine::sidevoice_engine::NoCredentials;
use sidevoice_desktop_engine::NativeEngines;
use sidevoice_voice::{AudioIo, NativeDevices};

use super::super::models::real_call::Fixture;
use super::super::models::{EngineLoader, Halves};
use super::{Applied, LaneAudio, Page, VoiceState};

/// The host's idle minute for the measurement; a stopped call's models stay `idle_unload_minutes` of them.
const MINUTE: Duration = Duration::from_secs(2);
/// How long to wait after the last call for the idle keep to run out and the models to be freed.
const SETTLE: Duration = Duration::from_secs(8);
/// Calls made back to back, and changes made to one call's settings.
const CALLS: usize = 10;
const CHANGES: usize = 12;

/// The recorded speaker and a speaker that plays at once, one lane per call.
#[derive(Clone, Default)]
struct Audio;

impl LaneAudio for Audio {
    type Port = ();

    fn attach(&self) -> (Box<dyn AudioIo>, ()) {
        (Box::new(Fixture { sink: None, stop: Arc::default(), played: Arc::default() }), ())
    }

    fn route_input(&self, _: Option<&()>) -> impl Future<Output = Result<(), String>> + Send {
        std::future::ready(Ok(()))
    }
}

/// What reached the page.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Value>>>);

impl Seen {
    fn page(&self) -> Page {
        let seen = Arc::clone(&self.0);
        Arc::new(move |event| seen.lock().unwrap().push(event))
    }

    fn has(&self, kind: &str, check: impl Fn(&Value) -> bool) -> bool {
        self.0.lock().unwrap().iter().any(|event| event["type"] == kind && check(&event["data"]))
    }

    /// Waits (the first run downloads the models) for the page to be told something; an error of the call fails.
    async fn until(&self, what: &str, done: impl Fn(&Seen) -> bool) {
        let seen = self.clone();
        let waited = tokio::time::timeout(Duration::from_secs(900), async move {
            while !done(&seen) {
                if let Some(error) = seen.0.lock().unwrap().iter().find(|event| event["type"] == "error") {
                    panic!("the call failed while waiting for {what}: {error}");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        waited.await.unwrap_or_else(|_| panic!("in time: {what}"));
    }
}

/// The settings of variant `which`, as the page would send them: both halves, the same with another language and
/// voice, the input half alone, the output half alone. The models are the same in each; what changes is which are
/// loaded and the configuration.
fn settings(which: usize, build: &str) -> VoiceSettings {
    let language = if which == 1 { "es" } else { "en" };
    let stt = json!({"catalog": "local", "model": "whisper-tiny", "language": language, "build": build});
    let input = json!({"stt": stt, "patience": "fast"});
    let tts = json!({"catalog": "local", "model": "kokoro-82m-v1.0"});
    let output = if which == 1 { json!({"tts": tts, "voice": "af_heart", "speed": 0.9}) } else { json!({"tts": tts}) };
    let mut settings = json!({"idle_unload_minutes": 1});
    if which != 3 {
        settings["input"] = input;
    }
    if which != 2 {
        settings["output"] = output;
    }
    serde_json::from_value(settings).unwrap()
}

/// One call on the host, as the page makes it: the lane, the start, a heard sentence, a spoken reply, the stop and the
/// release. `live`, when given, is told once the models are loaded and have worked (before the stop).
async fn call(
    state: &VoiceState<Audio>,
    catalogs: &[CatalogCandidates],
    loader: &Arc<dyn super::super::models::Loader>,
    build: &str,
    key: &str,
    live: Option<&str>,
) {
    let seen = Seen::default();
    state.create_call(key.into()).await.unwrap();
    let settings = settings(0, build);
    let choice = choose(&settings, catalogs).unwrap();
    let applied = Applied { halves: Halves::new(loader, &choice, None), settings };
    state.open(key, applied, false, seen.page()).await.unwrap();
    seen.until("a heard turn", |seen| seen.has("turn", |turn| turn["phase"] == "finished")).await;
    state
        .say(key, "reply".into(), "The tests pass on every platform.".into(), Some("en".into()), seen.page())
        .await
        .unwrap();
    seen.until("the reply said", |seen| seen.has("say", |say| say["key"] == "reply" && say["event"]["type"] == "done"))
        .await;
    if let Some(phase) = live {
        report(phase).await;
    }
    state.stop(key).await.unwrap();
    state.dispose_call(key).await.unwrap();
}

/// The sample, off the async workers (it runs `vmmap`).
async fn report(phase: &str) {
    let phase = phase.to_owned();
    tokio::task::spawn_blocking(move || crate::memory::report(&phase)).await.unwrap();
}

#[test]
#[ignore = "downloads real models and takes minutes; `cargo xtask probe voice-memory` runs it"]
fn the_footprint_stays_flat_over_calls_and_changes() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    let kept = std::env::var_os("SIDEVOICE_REAL_CALL_MODELS").map(std::path::PathBuf::from);
    let root =
        kept.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("sidevoice-memory-{}", std::process::id())));
    let engines = NativeEngines::new(&root, NoCredentials, runtime.handle().clone()).unwrap();
    let views = engines.catalogs().unwrap();
    let catalogs: Vec<CatalogCandidates> = views.iter().map(crate::voice::candidates).collect();
    let build = views[0]
        .models
        .iter()
        .find(|m| m.id == "whisper-tiny")
        .and_then(|m| m.local.as_ref())
        .and_then(|local| local.recommended_build.clone())
        .expect("a whisper-tiny build runs here");

    runtime.block_on(async {
        let mut state = VoiceState::new(Audio, NativeDevices::default());
        state.minute = MINUTE;
        let loader = EngineLoader::shared(engines.engine(), Arc::clone(&state.model_cache));

        // The engine is up and its catalogues listed; no model is loaded.
        report("idle").await;

        // A call with the models loaded; then the call ends and its idle minute runs out.
        call(&state, &catalogs, &loader, &build, "first", Some("call-live")).await;
        tokio::time::sleep(SETTLE).await;
        report("call-ended").await;

        // Calls back to back, the next starting as the last one ends, sharing what is loaded.
        for n in 0..CALLS {
            call(&state, &catalogs, &loader, &build, &format!("call-{n}"), None).await;
        }
        tokio::time::sleep(SETTLE).await;
        report("calls-many").await;

        // One call whose settings change again and again: a half added and removed, another language and voice.
        let seen = Seen::default();
        state.create_call("changing".into()).await.unwrap();
        let first = settings(0, &build);
        let choice = choose(&first, &catalogs).unwrap();
        let mut applied = Applied { halves: Halves::new(&loader, &choice, None), settings: first };
        state.open("changing", applied.clone(), false, seen.page()).await.unwrap();
        seen.until("a heard turn", |seen| seen.has("turn", |turn| turn["phase"] == "finished")).await;
        for n in 1..=CHANGES {
            let next = settings(n % 4, &build);
            let choice = choose(&next, &catalogs).unwrap();
            applied = Applied { halves: Halves::new(&loader, &choice, Some(&applied.halves)), settings: next };
            state.update("changing", applied.clone()).await.unwrap();
            // The change takes effect at its safe point, models loading while the call goes on.
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        report("changes-live").await;
        state.stop("changing").await.unwrap();
        state.dispose_call("changing").await.unwrap();
        tokio::time::sleep(SETTLE).await;
        report("changes-ended").await;

        // Models loaded ahead, again and again, as the page does when the person changes voice settings (`voice_preload`):
        // each is held for the idle minutes and then let go. By half, to tell which one stays.
        for (name, which) in [("preload-input", 2), ("preload-output", 3), ("preload-both", 0)] {
            for n in 0..CHANGES * 2 {
                let next = settings(if which == 0 { n % 2 } else { which }, &build);
                let choice = choose(&next, &catalogs).unwrap();
                state.preload(Halves::new(&loader, &choice, None), 1).await.unwrap();
            }
            report(&format!("{name}-live")).await;
            tokio::time::sleep(SETTLE).await;
            report(&format!("{name}-ended")).await;
        }
    });
    drop(engines);
    if kept.is_none() {
        let _ = std::fs::remove_dir_all(&root);
    }
}
