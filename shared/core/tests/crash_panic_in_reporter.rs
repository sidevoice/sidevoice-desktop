//! A panic inside the reporter, while the log's panic hook is handing it a panic, must not abort the process (a panic in a
//! panic hook cannot be caught) and must not hold the dying thread up. Its own test binary: the hook is the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::crash::{self, Config, Post, PostError, Reporter};
use sidevoice_shared::logging::{self, Resource};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Breaks(AtomicUsize);

impl Post for Breaks {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("the transport broke inside the panic hook");
    }
}

#[test]
fn a_reporter_that_panics_inside_the_hook_does_not_take_the_process_down() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Breaks(AtomicUsize::new(0)));
    logging::init(&["sidevoice_shared", logging::PANIC_TARGET], None, |_| {});
    logging::announce(Resource::of("canary-app", "9.9.9", "linux", "x86_64"));
    let reporter = Reporter::start(Config {
        resource: Resource::of("canary-app", "9.9.9", "linux", "x86_64"),
        dsn: None,
        dir: dir.path().to_owned(),
        post: Some(post.clone()),
    });
    crash::report_panics(reporter);
    for _ in 0..3 {
        let started = Instant::now();
        let thread = std::thread::spawn(|| panic!("an app panic"));
        assert!(thread.join().is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    assert!(post.0.load(Ordering::SeqCst) >= 1, "the reporter was reached");
}
