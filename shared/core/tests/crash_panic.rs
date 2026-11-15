//! A panic reaches the crash reporter through the log's panic hook (sidevoice-desktop#523): a literal message as it is, a
//! formatted one only as the marker, and nothing of its text anywhere in what is sent. Its own test binary and one test:
//! the hook and the subscriber are the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::crash::{self, Config, Post, PostError, Reporter};
use sidevoice_shared::logging::{self, Resource};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Post for Recorder {
    fn post(&self, envelope: Vec<u8>, _budget: std::time::Duration) -> Result<(), PostError> {
        self.0.lock().unwrap().push(String::from_utf8_lossy(&envelope).into_owned());
        Ok(())
    }
}

#[test]
fn a_panic_is_reported_as_a_literal_or_the_marker_and_never_as_its_formatted_text() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    logging::init(&["sidevoice_shared", logging::PANIC_TARGET], None, |_| {});
    logging::announce(Resource::of("canary-app", "9.9.9", "linux", "x86_64"));
    let reporter = Reporter::start(Config {
        resource: Resource::of("canary-app", "9.9.9", "linux", "x86_64"),
        dsn: None,
        dir: dir.path().to_owned(),
        post: Some(post.clone()),
    });
    crash::report_panics(reporter.clone());

    let literal = std::thread::Builder::new()
        .name("literal-panic".into())
        .spawn(|| panic!("literal words that may be sent"))
        .unwrap();
    assert!(literal.join().is_err());
    let formatted = std::thread::Builder::new()
        .name("formatted-panic".into())
        .spawn(|| {
            let result: Result<(), String> =
                std::hint::black_box(Err("/home/canary-user-7f3a/canary-secret-7f3a".into()));
            result.unwrap();
        })
        .unwrap();
    assert!(formatted.join().is_err());

    let sent = post.0.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].contains("literal words that may be sent"));
    assert!(sent[1].contains("<formatted panic message omitted>"));
    for envelope in &sent {
        assert!(!envelope.contains("canary-secret") && !envelope.contains("canary-user"), "{envelope}");
        assert!(envelope.contains("\"release\":\"canary-app@9.9.9\""), "{envelope}");
    }

    // After "never", the next panic sends nothing.
    reporter.set_mode(crash::Mode::Never).unwrap();
    let after = std::thread::spawn(|| panic!("after never"));
    assert!(after.join().is_err());
    assert_eq!(post.0.lock().unwrap().len(), 2);
}
