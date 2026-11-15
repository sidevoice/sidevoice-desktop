//! "Never breaks the app": whatever goes wrong inside the reporter (a transport that panics, an unreachable endpoint, a
//! settings directory that is not one, a machine that answers garbage, a send that finds the queue full), the app-facing
//! calls return normally.

use super::event;
use super::machine::{self, Machine, MachineError};
use super::reporter::{Config, Reporter, SendError};
use super::transport::{Gate, Post, PostError, MAX_IN_FLIGHT};
use super::{MachineRecord, Mode};
use crate::logging::Resource;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const RESOURCE: Resource = Resource {
    service_name: "sidevoice-desktop",
    service_version: "1.2.3",
    os_type: "linux",
    os_name: "Linux",
    host_arch: "x86_64",
};

fn config(dir: &std::path::Path, dsn: Option<&str>, post: Option<Arc<dyn Post>>) -> Config {
    Config { resource: RESOURCE, dsn: dsn.map(str::to_owned), dir: dir.to_owned(), post }
}

fn record() -> MachineRecord {
    MachineRecord::parse(&valid_row(event::random_uuid())).unwrap()
}

fn valid_row(id: sentry::types::Uuid) -> Value {
    json!({
        "id": id.hyphenated().to_string(), "piece": "core", "version": "0.4.1", "os": "linux", "arch": "x86_64",
        "first_at": "2026-11-14T10:00:00.000Z", "last_at": "2026-11-14T10:05:00.000Z", "count": 1,
        "exception": {"type": "panic", "message": "a literal"},
        "backtrace": ["core::foo"], "log_tail": []
    })
}

struct PanickingPost(AtomicBool);

impl Post for PanickingPost {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        if self.0.load(Ordering::SeqCst) {
            panic!("the transport broke");
        }
        Ok(())
    }
}

#[test]
fn an_unreachable_endpoint_is_a_failed_send_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing listens on port 1 of the loopback: the connection is refused.
    let reporter = Reporter::start(config(dir.path(), Some("https://public@127.0.0.1:1/1"), None));
    assert!(reporter.is_active());
    let started = Instant::now();
    reporter.capture_panic(Some("p"), "main");
    assert_eq!(reporter.send_machine(&record()), Err(SendError::Failed));
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
}

#[test]
fn a_transport_that_panics_is_contained_and_the_reporter_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(PanickingPost(AtomicBool::new(true)));
    let reporter = Reporter::start(config(dir.path(), None, Some(post.clone())));
    reporter.capture_panic(Some("p"), "main");
    assert_eq!(
        reporter.send_machine(&record()),
        Err(SendError::Failed),
        "a record whose send panicked is not delivered"
    );
    post.0.store(false, Ordering::SeqCst);
    assert_eq!(reporter.send_machine(&record()), Ok(()), "and the next send works");
    assert!(reporter.set_mode(Mode::Never).is_ok());
}

#[test]
fn a_settings_directory_that_is_not_one_leaves_reporting_off_and_the_app_working() {
    let dir = tempfile::tempdir().unwrap();
    // A regular file where the directory should be: nothing can be read, nothing can be written.
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let post = Arc::new(PanickingPost(AtomicBool::new(false)));
    let reporter = Reporter::start(config(&file, None, Some(post)));
    assert_eq!(reporter.mode(), Mode::Never, "an unreadable setting is never for this run");
    assert!(!reporter.is_active());
    reporter.capture_panic(Some("p"), "main");
    assert_eq!(reporter.installation_id(), None);
    assert_eq!(reporter.send_machine(&record()), Err(SendError::Disabled));
    assert!(reporter.set_mode(Mode::Auto).is_err(), "the choice cannot be kept, and says so");
    assert!(!reporter.is_active());

    // A garbage file in a real directory, and a directory with no way to write.
    let broken = tempfile::tempdir().unwrap();
    std::fs::write(broken.path().join(super::settings::FILE), vec![0xff; 100]).unwrap();
    let reporter = Reporter::start(config(broken.path(), None, None));
    assert_eq!(reporter.mode(), Mode::Never);
    reporter.capture_panic(None, "main");

    // The reporter that could not be had at all.
    let none = Reporter::unavailable(RESOURCE);
    none.capture_panic(None, "main");
    assert_eq!(none.installation_id(), None);
    assert!(none.set_mode(Mode::Auto).is_err());
    assert_eq!(none.mode(), Mode::Never);
}

#[test]
fn a_bad_dsn_is_reporting_off_for_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let reporter = Reporter::start(config(dir.path(), Some("not a dsn"), None));
    assert!(!reporter.is_active());
    reporter.capture_panic(None, "main");
    assert_eq!(reporter.send_machine(&record()), Err(SendError::Disabled));
    assert_eq!(reporter.mode(), Mode::Auto, "the setting itself still works");
}

#[test]
fn the_background_start_goes_live_and_never_overtakes_never() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(PanickingPost(AtomicBool::new(false)));
    let reporter = Reporter::start_in_background(config(dir.path(), None, Some(post.clone())));
    let until = Instant::now() + Duration::from_secs(5);
    while !reporter.is_active() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(reporter.is_active());
    // Chosen right away, before the thread could have run: never wins whichever comes first.
    for _ in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let reporter = Reporter::start_in_background(config(dir.path(), None, Some(post.clone())));
        reporter.set_mode(Mode::Never).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        assert!(!reporter.is_active());
    }
}

struct Garbage {
    rows: Vec<Value>,
    panics: bool,
    acknowledged: AtomicUsize,
}

impl Machine for Garbage {
    fn list(&self) -> Result<Vec<Value>, MachineError> {
        if self.panics {
            panic!("the machine's client broke");
        }
        Ok(self.rows.clone())
    }

    fn acknowledge(&self, _id: &str) -> Result<(), MachineError> {
        self.acknowledged.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn an_oversized_or_garbage_machine_answer_is_harmless() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(PanickingPost(AtomicBool::new(false)));
    let reporter = Reporter::start(config(dir.path(), None, Some(post)));
    let huge = "x".repeat(2 * 1024 * 1024);
    let mut rows = vec![
        Value::Null,
        json!(7),
        json!("text"),
        json!([[[[]]]]),
        json!({"id": huge, "piece": "core"}),
        json!({"id": 5, "exception": {"type": huge}}),
        json!({"id": "3f2b8c1e-5a47-4d1b-9c33-0a1b2c3d4e5f", "piece": "core", "backtrace": vec![huge.clone(); 100]}),
        json!({"id": "é€", "log_tail": ["\u{0}\u{1}"]}),
    ];
    rows.extend((0..10_000).map(|n| json!({"id": n})));
    let machine = Garbage { rows, panics: false, acknowledged: AtomicUsize::new(0) };
    let done = machine::sync(&reporter, &machine);
    assert_eq!(
        (done.sent, machine.acknowledged.load(Ordering::SeqCst)),
        (0, 0),
        "nothing garbage is sent or acknowledged"
    );

    // A machine that lists more valid records than a pass takes is read as far as the cap.
    let rows: Vec<Value> = (0..200).map(|_| valid_row(event::random_uuid())).collect();
    let machine = Garbage { rows, panics: false, acknowledged: AtomicUsize::new(0) };
    let done = machine::sync(&reporter, &machine);
    assert_eq!(done.sent, 50, "{done:?}");

    // One whose client panics, and one that does not answer.
    let machine = Garbage { rows: vec![], panics: true, acknowledged: AtomicUsize::new(0) };
    assert_eq!(machine::sync(&reporter, &machine), machine::Synced::default());
}

#[test]
fn a_tail_line_of_any_shape_is_judged_without_panicking() {
    for line in [
        "",
        " ",
        "-1.000s",
        "-1.000s ",
        "-1.000s WARN",
        "-1.000s WARN é{",
        "-1.000s WARN a{\"é\":1}",
        "+0.000s ERROR \u{a0}x{}",
        "é -1.000s WARN x: y",
    ] {
        let _ = super::tail::is_tail_line(line, machine::Piece::Core);
    }
}

/// A transport that holds every POST until it is released.
#[derive(Default)]
struct Blocking {
    calls: AtomicUsize,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Post for Blocking {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
        Ok(())
    }
}

#[test]
fn a_full_queue_drops_the_send_and_returns() {
    let post = Arc::new(Blocking::default());
    let gate = Arc::new(Gate::new(post.clone()));
    let total = MAX_IN_FLIGHT + 3;
    let threads: Vec<_> = (0..total)
        .map(|_| {
            let gate = gate.clone();
            std::thread::spawn(move || {
                sentry::Transport::send_envelope(&*gate, event::machine_record(&record()).into())
            })
        })
        .collect();
    // The ones that found the queue full come back at once; the others wait on the network.
    let until = Instant::now() + Duration::from_secs(5);
    while threads.iter().filter(|t| t.is_finished()).count() < total - MAX_IN_FLIGHT && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(threads.iter().filter(|t| t.is_finished()).count(), total - MAX_IN_FLIGHT);
    assert_eq!(post.calls.load(Ordering::SeqCst), MAX_IN_FLIGHT, "no more than the bound is ever under way");
    *post.released.lock().unwrap() = true;
    post.wake.notify_all();
    for thread in threads {
        thread.join().unwrap();
    }
}

struct Reentrant {
    reporter: OnceLock<Arc<Reporter>>,
    calls: AtomicUsize,
}

impl Post for Reentrant {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The reporter reporting a panic of its own, from inside a send.
        if let Some(reporter) = self.reporter.get() {
            reporter.capture_panic(Some("again"), "inner");
        }
        Ok(())
    }
}

#[test]
fn a_panic_reported_from_inside_a_report_does_not_recurse_or_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Reentrant { reporter: OnceLock::new(), calls: AtomicUsize::new(0) });
    let reporter = Reporter::start(config(dir.path(), None, Some(post.clone())));
    post.reporter.set(reporter.clone()).ok().unwrap();
    reporter.capture_panic(Some("outer"), "main");
    assert_eq!(post.calls.load(Ordering::SeqCst), 1);
    reporter.capture_panic(Some("next"), "main");
    assert_eq!(post.calls.load(Ordering::SeqCst), 2, "and the guard is released afterwards");
}

#[test]
fn a_run_reports_a_bounded_number_of_own_panics() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Reentrant { reporter: OnceLock::new(), calls: AtomicUsize::new(0) });
    let reporter = Reporter::start(config(dir.path(), None, Some(post.clone())));
    for _ in 0..100 {
        reporter.capture_panic(Some("loop"), "main");
    }
    assert_eq!(post.calls.load(Ordering::SeqCst), 10);
}

#[test]
fn the_poll_loop_survives_passes_that_panic_and_backs_off() {
    let dir = tempfile::tempdir().unwrap();
    let reporter = Reporter::start(config(dir.path(), None, None));
    let runs = AtomicUsize::new(0);
    machine::poll_while(
        &reporter,
        Duration::ZERO,
        Duration::from_millis(1),
        || runs.load(Ordering::SeqCst) < 6,
        |_| {
            if runs.fetch_add(1, Ordering::SeqCst) < 3 {
                panic!("a pass broke");
            }
        },
    );
    assert_eq!(runs.load(Ordering::SeqCst), 6, "the loop went on after each panic");
    let every = Duration::from_secs(300);
    assert_eq!(machine::backoff(every, 1), every * 2);
    assert!(machine::backoff(every, 40) <= Duration::from_secs(3600));
    assert_eq!(machine::backoff(Duration::MAX, u32::MAX), Duration::MAX, "no overflow");
}
