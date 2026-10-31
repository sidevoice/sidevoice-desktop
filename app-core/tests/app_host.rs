//! The application host as a shell that is not Tauri uses it: the three hooks are fakes, the audio is a stand-in, the
//! runtime is a handle this test owns and the test drives the host from a thread with no Tokio context at all (`block_on`
//! below), as a shell with an executor of its own would: what the host spawns or times must come from the handle it was
//! given. Nothing of a webview is in this crate's closure, so a test that drives `AppHost` through its public API is the
//! proof that a shell is only transport. (What starting a call does with the real models
//! is `voice::host`'s and `voice::models`' own tests, and CI's real-model call.)

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sidevoice_app_core::app::{
    Admission, AlwaysAdmit, AppHost, EventSink, Hooks, HostConfig, KeyError, Lease, PageGeneration, ProviderKeys,
};
use sidevoice_app_core::sidevoice_voice::{AudioIo, IoSink, NativeDevices};
use sidevoice_app_core::voice::host::LaneAudio;
use sidevoice_app_core::voice::{SettingsChange, VoiceSettings};

/// A reset that can be under way, and the leases held while it is not.
#[derive(Default)]
struct Gate {
    resetting: AtomicBool,
    live: Arc<AtomicUsize>,
}

struct Held(Arc<AtomicUsize>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Admission for Gate {
    fn admit(&self) -> Option<Lease> {
        if self.resetting.load(Ordering::SeqCst) {
            return None;
        }
        self.live.fetch_add(1, Ordering::SeqCst);
        Some(Lease::new(Held(Arc::clone(&self.live))))
    }
}

#[derive(Default)]
struct Page {
    events: Mutex<Vec<Value>>,
    pages: Mutex<Vec<PageGeneration>>,
}

impl EventSink for Page {
    fn event(&self, page: PageGeneration, event: Value) {
        self.pages.lock().unwrap().push(page);
        self.events.lock().unwrap().push(event);
    }
}

/// The shell's store, which also says how many leases were live whenever it was written.
struct Store {
    keys: Mutex<HashMap<String, String>>,
    live: Arc<AtomicUsize>,
    live_when_written: Mutex<Vec<usize>>,
    fails: Mutex<Option<KeyError>>,
}

impl ProviderKeys for Store {
    fn valid(&self, provider: &str) -> bool {
        (1..=32).contains(&provider.len()) && provider.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    }

    fn read(&self, provider: &str) -> Result<Option<String>, KeyError> {
        if let Some(error) = *self.fails.lock().unwrap() {
            return Err(error);
        }
        Ok(self.keys.lock().unwrap().get(provider).cloned())
    }

    fn write(&self, provider: &str, key: Option<&str>) -> Result<(), KeyError> {
        self.live_when_written.lock().unwrap().push(self.live.load(Ordering::SeqCst));
        if let Some(error) = *self.fails.lock().unwrap() {
            return Err(error);
        }
        let mut keys = self.keys.lock().unwrap();
        match key {
            Some(key) => keys.insert(provider.to_owned(), key.to_owned()),
            None => keys.remove(provider),
        };
        Ok(())
    }
}

/// Audio that is never opened: a lane only has to exist.
#[derive(Clone)]
struct Silent;

struct NoIo;

impl AudioIo for NoIo {
    fn start(&mut self, _sink: IoSink, _input: bool) -> Result<(), String> {
        Ok(())
    }

    fn set_input(&mut self, _input: bool) {}

    fn play(&mut self, _utterance: &str, _chunk: usize, _samples: Vec<f32>, _sample_rate: u32) {}

    fn stop_playback(&mut self) {}

    fn stop(&mut self) {}
}

impl LaneAudio for Silent {
    type Port = ();

    fn attach(&self) -> (Box<dyn AudioIo>, ()) {
        (Box::new(NoIo), ())
    }

    fn route_input(&self, _target: Option<&()>) -> impl Future<Output = Result<(), String>> + Send {
        std::future::ready(Ok(()))
    }
}

/// Drives `future` on this thread with no Tokio context at all: the shell that calls the host from an executor of its
/// own. Whatever the host spawns, times or asks the current runtime for must come from the handle it was given, or this
/// panics (there is no reactor running).
fn block_on<F: Future>(future: F) -> F::Output {
    struct Wake(std::thread::Thread);

    impl std::task::Wake for Wake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = std::task::Waker::from(Arc::new(Wake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => return output,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}

struct Shell {
    _runtime: tokio::runtime::Runtime,
    host: AppHost<Silent>,
    gate: Arc<Gate>,
    page: Arc<Page>,
    store: Arc<Store>,
    root: PathBuf,
}

impl Shell {
    fn new(name: &str) -> Shell {
        let root = std::env::temp_dir().join(format!("sidevoice-app-host-{name}-{}", std::process::id()));
        Shell::on(name, root)
    }

    fn on(_name: &str, root: PathBuf) -> Shell {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let gate = Arc::new(Gate::default());
        let page = Arc::new(Page::default());
        let store = Arc::new(Store {
            keys: Mutex::default(),
            live: Arc::clone(&gate.live),
            live_when_written: Mutex::default(),
            fails: Mutex::default(),
        });
        let hooks = Hooks {
            admission: Arc::clone(&gate) as Arc<dyn Admission>,
            events: Arc::clone(&page) as Arc<dyn EventSink>,
            keys: Arc::clone(&store) as Arc<dyn ProviderKeys>,
        };
        let host = AppHost::with_audio(
            HostConfig { model_root: root.clone() },
            runtime.handle().clone(),
            hooks,
            Silent,
            NativeDevices::default(),
        );
        Shell { _runtime: runtime, host, gate, page, store, root }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_file(&self.root);
    }
}

fn key_of(refusal: &impl serde::Serialize) -> String {
    serde_json::to_value(refusal).unwrap()["key"].as_str().unwrap().to_owned()
}

fn settings(value: Value) -> VoiceSettings {
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_reset_under_way_refuses_what_uses_the_data() {
    let shell = Shell::new("reset");
    let host = &shell.host;
    let page = host.page();
    block_on(async {
        host.create_call(page, "a".into()).await.unwrap();
        shell.gate.resetting.store(true, Ordering::SeqCst);

        let call = settings(json!({"output": {"tts": {"catalog": "local", "model": "kokoro-82m-v1.0"}}}));
        assert_eq!(key_of(&host.start(page, "a", call.clone(), false).await.unwrap_err()), "reset.busy");
        assert_eq!(key_of(&host.ready(page, "a").await.unwrap_err()), "reset.busy");
        assert_eq!(key_of(&host.preload(page, call).await.unwrap_err()), "reset.busy");
        assert_eq!(key_of(&host.update(page, "a", SettingsChange::default()).await.unwrap_err()), "reset.busy");
        assert_eq!(key_of(&host.install("m".into(), None, None, "job".into()).await.unwrap_err()), "reset_busy");
        assert_eq!(key_of(&host.uninstall("m".into()).await.unwrap_err()), "reset_busy");
        assert_eq!(key_of(&host.set_credential("openai".into(), Some("k".into())).await.unwrap_err()), "reset_busy");
        assert!(shell.store.live_when_written.lock().unwrap().is_empty(), "nothing was written");

        // What does not use the data is not held off, and the reset ends: the work is admitted again.
        host.mute(page, "a", true).await.unwrap();
        assert!(host.catalogs().await.is_ok());
        shell.gate.resetting.store(false, Ordering::SeqCst);
        assert_eq!(key_of(&host.set_credential("not-a-provider".into(), None).await.unwrap_err()), "catalog_not_found");
    });
    assert_eq!(shell.gate.live.load(Ordering::SeqCst), 0, "no lease outlives its work");
}

#[test]
fn a_lease_is_held_on_the_blocking_thread_until_the_work_ends() {
    let shell = Shell::new("lease");
    block_on(async {
        // The store is written on a blocking thread, with the lease the command took still live; the key is trimmed. The
        // engine then has no such provider, which is the answer here: the write came first, in order.
        let answer = shell.host.set_credential("not-a-provider".into(), Some("  sk-secret  ".into())).await;
        assert_eq!(key_of(&answer.unwrap_err()), "catalog_not_found");
        assert_eq!(*shell.store.live_when_written.lock().unwrap(), [1]);
        assert_eq!(shell.store.keys.lock().unwrap().get("not-a-provider").map(String::as_str), Some("sk-secret"));
    });
    assert_eq!(shell.gate.live.load(Ordering::SeqCst), 0);
}

#[test]
fn provider_keys_go_through_the_shells_store_and_nothing_of_a_failure_is_told() {
    let shell = Shell::new("keys");
    let host = &shell.host;
    block_on(async {
        // Blank removes; an id the store does not take is refused as such, before the store is asked.
        shell.store.keys.lock().unwrap().insert("not-a-provider".into(), "sk-secret".into());
        assert!(host.has_credential("not-a-provider".into()).await.unwrap());
        assert_eq!(
            key_of(&host.set_credential("not-a-provider".into(), Some("  ".into())).await.unwrap_err()),
            "catalog_not_found"
        );
        assert!(!host.has_credential("not-a-provider".into()).await.unwrap());
        assert_eq!(key_of(&host.has_credential("Bad Id".into()).await.unwrap_err()), "provider_invalid");
        assert_eq!(key_of(&host.test_credential("Bad Id".into(), "k".into()).await.unwrap_err()), "provider_invalid");

        // A failure of the store is told in a category, and never carries the key.
        for (error, key, reason) in [
            (KeyError::Unavailable, "credentials_unavailable", None),
            (KeyError::Failed, "credentials_failed", None),
            (KeyError::TooLong, "credentials_failed", Some("too_long")),
        ] {
            *shell.store.fails.lock().unwrap() = Some(error);
            let refused = host.set_credential("not-a-provider".into(), Some("sk-secret".into())).await.unwrap_err();
            let told = serde_json::to_value(&refused).unwrap();
            assert_eq!(told["key"], key);
            assert_eq!(told.get("reason").and_then(Value::as_str), reason);
            assert!(!told.to_string().contains("sk-secret"), "{told}");
        }
    });
}

#[test]
fn a_call_is_refused_what_the_engine_cannot_name_and_its_events_reach_the_sink() {
    let shell = Shell::new("call");
    let host = &shell.host;
    let page = host.page();
    block_on(async {
        host.create_call(page, "a".into()).await.unwrap();
        assert_eq!(key_of(&host.create_call(page, "a".into()).await.unwrap_err()), "bad_request", "one lane per key");
        assert_eq!(key_of(&host.stop(page, "nobody").await.unwrap_err()), "bad_request");

        // The catalogues are the engine's; a catalogue it does not have is refused with the page's code, nothing loaded.
        let unknown = settings(json!({"input": {"stt": {"catalog": "nowhere", "model": "m"}}}));
        let refused = host.start(page, "a", unknown, false).await.unwrap_err();
        assert_eq!(key_of(&refused), "voice_catalog_not_found");
        assert_eq!(serde_json::to_value(&refused).unwrap()["code"], "catalog-not-found");
        let none = settings(json!({}));
        assert_eq!(key_of(&host.start(page, "a", none, false).await.unwrap_err()), "voice_halves_missing");

        // Before the call starts there is nothing to say it on: the saying ends at once, told to the page through the sink.
        host.say(page, "a", "s1".into(), "hello".into(), None).await.unwrap();
        let told = shell.page.events.lock().unwrap().clone();
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(
            (&told[0]["key"], &told[0]["type"], &told[0]["data"]["key"]),
            (&json!("a"), &json!("say"), &json!("s1"))
        );
        assert_eq!(told[0]["data"]["event"]["type"], "done");

        assert!(!host.stop(page, "a").await.unwrap(), "stopping before the call starts answers at once, with false");
        host.dispose_call(page, "a").await.unwrap();
    });
}

#[test]
fn the_engine_answers_through_the_host() {
    let shell = Shell::new("engine");
    let host = &shell.host;
    block_on(async {
        let device = host.capabilities().unwrap();
        assert_eq!(device.runs, "native");
        let catalogs = host.catalogs().await.unwrap();
        assert_eq!(catalogs[0].id, "local", "the local catalogue first");
        assert!(host.installed().await.unwrap().is_empty(), "a fresh model root holds nothing");
        assert!(host.memory().unwrap().total_mb.is_some());
        assert_eq!(host.progress("nothing"), None);
        host.cancel("nothing");
        assert!(!host.installing());
        assert_eq!(
            key_of(&host.install("no-such-model".into(), None, None, "j".into()).await.unwrap_err()),
            "model_unknown"
        );
        assert!(!host.installing(), "a refused install leaves no job");
    });
}

#[test]
fn a_reloaded_page_owns_none_of_the_lanes() {
    let shell = Shell::new("reload");
    let host = &shell.host;
    let first = host.page();
    block_on(async {
        host.create_call(first, "a".into()).await.unwrap();
        let page = host.page_changed();
        // The old lanes are released in the background.
        let mut tries = 0;
        while host.route_input(page, Some("a".into())).await.is_ok() {
            tries += 1;
            assert!(tries < 100, "the lane was never released");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(key_of(&host.route_input(page, Some("a".into())).await.unwrap_err()), "bad_request");
        // The same key can be taken again by the new page.
        host.create_call(page, "a".into()).await.unwrap();
    });
}

#[test]
fn a_host_without_an_engine_refuses_what_needs_one_and_still_runs_lanes() {
    // A file where the model root should be: the engine cannot start.
    let root = std::env::temp_dir().join(format!("sidevoice-app-host-noengine-{}", std::process::id()));
    std::fs::write(&root, "not a folder").unwrap();
    let shell = Shell::on("noengine", root);
    let host = &shell.host;
    let page = host.page();
    block_on(async {
        assert_eq!(key_of(&host.capabilities().unwrap_err()), "engine_unavailable");
        assert_eq!(key_of(&host.catalogs().await.unwrap_err()), "engine_unavailable");
        assert_eq!(key_of(&host.install("m".into(), None, None, "j".into()).await.unwrap_err()), "engine_unavailable");
        host.create_call(page, "a".into()).await.unwrap();
        let call = settings(json!({"output": {"tts": {"catalog": "local", "model": "kokoro-82m-v1.0"}}}));
        assert_eq!(key_of(&host.start(page, "a", call, false).await.unwrap_err()), "engine_unavailable");
        host.cancel("nothing");
        assert!(host.progress("nothing").is_none(), "jobs need no engine");
    });
}

#[test]
fn closing_releases_every_lane_and_answers_none_held() {
    let shell = Shell::new("close");
    let host = &shell.host;
    let page = host.page();
    block_on(async {
        host.create_call(page, "a".into()).await.unwrap();
        host.create_call(page, "b".into()).await.unwrap();
        assert_eq!(host.close(Duration::from_secs(5)).await, Vec::<String>::new());
        assert_eq!(key_of(&host.mute(page, "a", true).await.unwrap_err()), "bad_request", "no lane is left");
    });
}

#[test]
fn an_app_that_never_resets_admits_every_time() {
    let admission = AlwaysAdmit;
    let first = admission.admit();
    let second = admission.admit();
    assert!(first.is_some() && second.is_some());
}

#[test]
fn a_page_owns_its_lanes_and_the_old_ones_going_does_not_take_the_new_pages() {
    let shell = Shell::new("generations");
    let host = &shell.host;
    block_on(async {
        // The new page makes its lane at once, while the old page's are released in the background; run it over and over,
        // for the clearing to land before and after the new lane is made.
        for n in 0..40 {
            let (old, old_key, new_key) = (host.page(), format!("old-{n}"), format!("new-{n}"));
            host.create_call(old, old_key.clone()).await.unwrap();
            let new = host.page_changed();
            assert_ne!(old, new);
            host.create_call(new, new_key.clone()).await.unwrap();

            // What the old page still asks is refused, and changes nothing.
            let stale = host.mute(old, &old_key, true).await.unwrap_err();
            assert_eq!(key_of(&stale), "page_stale");
            assert_eq!(serde_json::to_value(&stale).unwrap()["code"], "page-stale");
            assert_eq!(key_of(&host.create_call(old, format!("late-{n}")).await.unwrap_err()), "page_stale");

            // The old lane goes; the new one is still there after.
            let mut tries = 0;
            while host.mute(new, &old_key, true).await.is_ok() {
                tries += 1;
                assert!(tries < 500, "the old page's lane was never released");
                std::thread::sleep(Duration::from_millis(10));
            }
            host.mute(new, &new_key, true).await.unwrap();
            host.dispose_call(new, &new_key).await.unwrap();
        }
    });
}

#[test]
fn what_reaches_the_sink_is_for_the_page_that_is_current() {
    let shell = Shell::new("sink");
    let host = &shell.host;
    block_on(async {
        let old = host.page();
        host.create_call(old, "a".into()).await.unwrap();
        host.say(old, "a", "one".into(), "hello".into(), None).await.unwrap();

        let new = host.page_changed();
        // The old page can say nothing more, and the new one is heard under its own number.
        assert_eq!(key_of(&host.say(old, "a", "two".into(), "hello".into(), None).await.unwrap_err()), "page_stale");
        host.create_call(new, "b".into()).await.unwrap();
        host.say(new, "b", "three".into(), "hello".into(), None).await.unwrap();
    });
    let told: Vec<(PageGeneration, Value)> = {
        let pages = shell.page.pages.lock().unwrap().clone();
        pages.into_iter().zip(shell.page.events.lock().unwrap().clone()).collect()
    };
    let who: Vec<_> = told.iter().map(|(page, event)| (page.0, event["key"].as_str().unwrap().to_owned())).collect();
    assert_eq!(who, [(0, "a".to_owned()), (1, "b".to_owned())]);
}

#[test]
fn a_ready_waiting_when_the_page_goes_is_answered_not_left_hanging() {
    let shell = Shell::new("settle");
    let host = shell.host.clone();
    let old = host.page();
    block_on(host.create_call(old, "a".into())).unwrap();
    let (answer, answered) = std::sync::mpsc::channel();
    let waiting = host.clone();
    std::thread::spawn(move || {
        let _ = answer.send(block_on(waiting.ready(old, "a")));
    });
    assert!(answered.recv_timeout(Duration::from_millis(200)).is_err(), "nothing makes it work");
    host.page_changed();
    let refused = answered.recv_timeout(Duration::from_secs(10)).expect("answered when the page went").unwrap_err();
    assert_eq!(serde_json::to_value(&refused).unwrap()["code"], "stopped");
}
