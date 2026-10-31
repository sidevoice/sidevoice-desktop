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
    fn event(&self, _event: Value) {}
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

fn host_on(runtime: &tokio::runtime::Runtime, name: &str) -> (AppHost<Silent>, Arc<Counted>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("sidevoice-app-{name}-{}", std::process::id()));
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
    (host, gate, root)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

/// A load whose caller gave up while it waited (for the catalogues, say): the app quits, and the load must neither
/// outlive `close` nor start native work after it.
#[test]
fn close_waits_for_a_load_the_caller_gave_up_and_it_does_not_start_loading() {
    let runtime = runtime();
    let (host, gate, root) = host_on(&runtime, "close-drains");
    let lease = gate.admit().unwrap();
    let (proceed, waiting) = std::sync::mpsc::channel::<()>();
    let (loaded, loading) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (started, answer) = (Arc::clone(&loaded), Arc::clone(&loading));
    let inside = host.clone();
    let load = host.detached(async move {
        let _lease = lease;
        // Waiting for the catalogues, on a blocking thread.
        tokio::task::spawn_blocking(move || waiting.recv().ok()).await.unwrap();
        let allowed = inside.loading_allowed();
        answer.store(allowed.is_ok(), Ordering::SeqCst);
        allowed?;
        started.store(true, Ordering::SeqCst);
        Ok::<(), Refusal>(())
    });
    // The caller polls it once, and gives up.
    let mut load = Box::pin(load);
    assert!(load.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    drop(load);

    // The app quits: `close` is under way while the load still waits.
    let closing = host.clone();
    let closed = std::thread::spawn(move || {
        let waiter = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let waited = Instant::now();
        let held = waiter.block_on(closing.close(Duration::from_secs(20)));
        (held, waited.elapsed())
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!closed.is_finished(), "close waits for the load");
    assert_eq!(gate.0.load(Ordering::SeqCst), 1);
    proceed.send(()).unwrap();
    let (held, _) = closed.join().unwrap();
    assert!(held.is_empty());

    assert!(!loading.load(Ordering::SeqCst), "the load asked, found the app quitting");
    assert!(!loaded.load(Ordering::SeqCst), "and loaded nothing");
    assert_eq!(gate.0.load(Ordering::SeqCst), 0, "its lease is let go before close returned");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_load_after_close_is_refused_and_one_that_will_not_end_is_given_up() {
    let runtime = runtime();
    let (host, gate, root) = host_on(&runtime, "close-gives-up");
    let lease = gate.admit().unwrap();
    let ended = Arc::new(AtomicBool::new(false));
    let still = Arc::clone(&ended);
    // A load that never ends by itself.
    let mut stuck = Box::pin(host.detached(async move {
        let _lease = lease;
        tokio::time::sleep(Duration::from_secs(3600)).await;
        still.store(true, Ordering::SeqCst);
        Ok::<(), Refusal>(())
    }));
    assert!(stuck.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());

    let closing = host.clone();
    let started = Instant::now();
    let held = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(closing.close(Duration::from_secs(2)))
    })
    .join()
    .unwrap();
    assert!(held.is_empty());
    assert!(started.elapsed() < Duration::from_secs(10), "bounded");
    assert!(!ended.load(Ordering::SeqCst));
    assert_eq!(gate.0.load(Ordering::SeqCst), 0, "given up, its lease let go");

    let mut late = Box::pin(host.detached(async { Ok::<(), Refusal>(()) }));
    let refused = match late.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        std::task::Poll::Ready(Err(refused)) => refused,
        other => panic!("{:?}", other.is_ready()),
    };
    assert_eq!(serde_json::to_value(&refused).unwrap()["key"], "shutting_down");
    assert!(host.loading_allowed().is_err());
    let _ = std::fs::remove_dir_all(&root);
}
