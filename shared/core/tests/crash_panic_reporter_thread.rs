//! A panic on a thread of the reporter (its sending thread, the transport's own: `SentryPost` runs the HTTP and TLS code on one)
//! is never recorded as a crash of the app, and does not keep the app's own next panic from being recorded. Its own test binary:
//! the hook is the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::crash::{self, Config, Post, PostError, Reporter};
use sidevoice_shared::logging::{self, Resource};
use std::sync::Arc;
use std::time::Duration;

struct Nothing;

impl Post for Nothing {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        Ok(())
    }
}

fn records(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join("crash-records")).map(|entries| entries.flatten().count()).unwrap_or(0)
}

#[test]
fn a_panic_on_a_thread_of_the_reporter_is_never_recorded_as_a_crash_of_the_app() {
    let dir = tempfile::tempdir().unwrap();
    logging::init(&["sidevoice_shared", logging::PANIC_TARGET], None, |_| {});
    let reporter = Reporter::start(Config {
        resource: Resource::of("canary-app", "9.9.9", "linux", "x86_64"),
        dsn: None,
        dir: dir.path().to_owned(),
        post: Some(Arc::new(Nothing)),
    });
    crash::report_panics(reporter);
    let thread = crash::reporter_thread("crash-post", || panic!("a panic in the HTTP code")).unwrap();
    assert!(thread.join().is_err());
    assert_eq!(records(dir.path()), 0, "not recorded");
    let app = std::thread::spawn(|| panic!("an app panic"));
    assert!(app.join().is_err());
    assert_eq!(records(dir.path()), 1, "the app's own panic still is");
}
