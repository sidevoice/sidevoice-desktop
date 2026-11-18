//! A panic inside the reporter (its sending thread, a transport that breaks) must not take the process down, must not be
//! recorded as a crash of the app (no loop), and must not keep the next real panic from being recorded. Its own test binary: the
//! hook is the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::crash::{self, Config, Post, PostError, Reporter};
use sidevoice_shared::logging::{self, Resource};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Breaks(AtomicUsize);

impl Post for Breaks {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("the transport broke inside the pass");
    }
}

struct Nothing;

impl Post for Nothing {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        Ok(())
    }
}

fn records(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("crash-records")).map(|entries| entries.flatten().count()).unwrap_or(0)
}

fn config(dir: &Path, post: Arc<dyn Post>) -> Config {
    Config {
        resource: Resource::of("canary-app", "9.9.9", "linux", "x86_64"),
        dsn: None,
        dir: dir.to_owned(),
        post: Some(post),
    }
}

#[test]
fn a_reporter_that_panics_does_not_take_the_process_down_is_not_recorded_and_does_not_block_the_next_record() {
    let dir = tempfile::tempdir().unwrap();
    logging::init(&["sidevoice_shared", logging::PANIC_TARGET], None, |_| {});
    let reporter = Reporter::start(config(dir.path(), Arc::new(Nothing)));
    crash::report_panics(reporter.clone());
    let one = std::thread::spawn(|| panic!("an app panic"));
    assert!(one.join().is_err());
    assert_eq!(records(dir.path()), 1);

    // The next start's pass meets a transport that panics: contained, and the panic is the reporter's, not the app's.
    let breaks = Arc::new(Breaks(AtomicUsize::new(0)));
    let next = Reporter::start(config(dir.path(), breaks.clone()));
    assert!(next.wait_idle(Duration::from_secs(10)));
    assert_eq!(breaks.0.load(Ordering::SeqCst), 1, "the transport was reached");
    assert_eq!(records(dir.path()), 1, "the record is still there, and the transport's panic made none");

    // A real panic still gets its record, and the hook is not held up.
    for _ in 0..2 {
        let started = Instant::now();
        let thread = std::thread::spawn(|| panic!("another app panic"));
        assert!(thread.join().is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
    assert_eq!(records(dir.path()), 3);
}
