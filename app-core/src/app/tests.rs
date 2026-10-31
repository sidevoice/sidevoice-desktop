use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Waker};
use std::time::{Duration, Instant};

use serde_json::Value;
use sidevoice_voice::{AudioIo, IoSink, NativeDevices};

use super::*;
use crate::voice::host::Refusal;

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

struct Silent;

impl LaneAudio for Silent {
    type Port = ();

    fn attach(&self) -> (Box<dyn AudioIo>, ()) {
        (Box::new(NoIo), ())
    }

    fn route_input(&self, _target: Option<&()>) -> impl Future<Output = Result<(), String>> + Send {
        std::future::ready(Ok(()))
    }
}

/// Counts the leases alive.
#[derive(Default)]
struct Counted(Arc<AtomicUsize>);

struct Held(Arc<AtomicUsize>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Admission for Counted {
    fn admit(&self) -> Option<Lease> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Some(Lease::new(Held(Arc::clone(&self.0))))
    }
}

struct Nowhere;

impl EventSink for Nowhere {
    fn event(&self, _page: PageGeneration, _event: Value) {}
}

impl ProviderKeys for Nowhere {
    fn valid(&self, _provider: &str) -> bool {
        true
    }

    fn read(&self, _provider: &str) -> Result<Option<String>, KeyError> {
        Ok(None)
    }

    fn write(&self, _provider: &str, _key: Option<&str>) -> Result<(), KeyError> {
        Ok(())
    }
}

#[test]
fn work_that_loads_models_keeps_its_lease_when_the_caller_gives_up_waiting() {
    let root = std::env::temp_dir().join(format!("sidevoice-app-detached-{}", std::process::id()));
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let gate = Arc::new(Counted::default());
    let hooks = Hooks {
        admission: Arc::clone(&gate) as Arc<dyn Admission>,
        events: Arc::new(Nowhere),
        keys: Arc::new(Nowhere),
    };
    let host = AppHost::with_audio(
        HostConfig { model_root: root.clone() },
        runtime.handle().clone(),
        hooks,
        Silent,
        NativeDevices::default(),
    );

    let lease = gate.admit().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let finished = Arc::clone(&done);
    let work = host.detached(async move {
        let _lease = lease;
        tokio::time::sleep(Duration::from_millis(200)).await;
        finished.store(true, Ordering::SeqCst);
        Ok::<(), Refusal>(())
    });
    // The caller waits once, outside the runtime, and gives up.
    let mut work = Box::pin(work);
    assert!(work.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(work);
    assert_eq!(gate.0.load(Ordering::SeqCst), 1, "the lease outlives the caller that stopped waiting");
    assert!(!done.load(Ordering::SeqCst));

    let until = Instant::now() + Duration::from_secs(10);
    while !done.load(Ordering::SeqCst) {
        assert!(Instant::now() < until, "the work never ended");
        std::thread::sleep(Duration::from_millis(10));
    }
    while gate.0.load(Ordering::SeqCst) != 0 {
        assert!(Instant::now() < until, "the lease was never let go");
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// What the sink was given, and for which page.
#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<(PageGeneration, Value)>>);

impl EventSink for Recording {
    fn event(&self, page: PageGeneration, event: Value) {
        self.0.lock().unwrap().push((page, event));
    }
}

#[test]
fn what_a_call_of_an_old_page_says_is_dropped_once_the_next_page_is_current() {
    let root = std::env::temp_dir().join(format!("sidevoice-app-sink-{}", std::process::id()));
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let recording = Arc::new(Recording::default());
    let hooks = Hooks {
        admission: Arc::new(AlwaysAdmit),
        events: Arc::clone(&recording) as Arc<dyn EventSink>,
        keys: Arc::new(Nowhere),
    };
    let host = AppHost::with_audio(
        HostConfig { model_root: root.clone() },
        runtime.handle().clone(),
        hooks,
        Silent,
        NativeDevices::default(),
    );

    let old = host.page();
    let from_old = host.page_sink(old);
    from_old(serde_json::json!({"n": 1}));
    let new = host.page_changed();
    // The idle state that answers a stop, a take: they can still be on their way when the next page comes.
    from_old(serde_json::json!({"n": 2}));
    host.page_sink(new)(serde_json::json!({"n": 3}));
    from_old(serde_json::json!({"n": 4}));

    let told: Vec<(u64, i64)> =
        recording.0.lock().unwrap().iter().map(|(page, event)| (page.0, event["n"].as_i64().unwrap())).collect();
    assert_eq!(told, [(old.0, 1), (new.0, 3)], "in order, each under its own page, and none of the old page's after");
    let _ = std::fs::remove_dir_all(&root);
}
