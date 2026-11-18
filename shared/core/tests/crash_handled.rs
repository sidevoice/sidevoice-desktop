//! Handled errors through the process's real subscriber: the layer `logging::init` installs, the reporter that starts the sender and
//! the app's list of codes. Only the `error` events of our own targets that carry one of the app's codes leave, once a type a day,
//! and nothing while the switch says never. Its own test binary: the subscriber is the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::crash::{self, Config, Mode, Post, PostError, Reporter};
use sidevoice_shared::error::SharedError;
use sidevoice_shared::logging::{self, Resource};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Recorder(Mutex<Vec<Vec<u8>>>);

impl Post for Recorder {
    fn post(&self, envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        self.0.lock().unwrap().push(envelope);
        Ok(())
    }
}

impl Recorder {
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    /// The `error.type` of each event sent, in order.
    fn types(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|envelope| {
                String::from_utf8_lossy(envelope)
                    .lines()
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .find_map(|value| value["exception"]["values"][0]["type"].as_str().map(str::to_owned))
                    .expect("an event with an exception")
            })
            .collect()
    }
}

fn wait_for(what: &str, done: impl Fn() -> bool) {
    for _ in 0..6000 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn handled_errors_leave_under_the_switch_and_only_with_the_apps_codes() {
    let dir = tempfile::tempdir().unwrap();
    let own = ["sidevoice_shared", logging::PANIC_TARGET, logging::BACKTRACE_TARGET, logging::WEBVIEW_TARGET];
    logging::init(&own, None, |_| {});
    let post = Arc::new(Recorder::default());
    let reporter = Reporter::start(Config {
        resource: Resource::of("canary-app", "9.9.9", "linux", "x86_64"),
        dsn: None,
        dir: dir.path().to_owned(),
        post: Some(post.clone()),
    });
    let held = SharedError::VoiceModelsHeld.code();
    let failed = SharedError::NativeEngineFailed.code();

    // Until the app starts it nothing is watched, and nothing counted.
    tracing::event!(target: "sidevoice_shared::early", tracing::Level::ERROR, error.type = held, "before the reporter takes over");
    assert_eq!(crash::unreported_errors(), 0);
    crash::report_handled_errors(reporter.clone(), SharedError::codes());

    tracing::event!(target: "sidevoice_shared::call", tracing::Level::ERROR, error.type = held, "the first of its type");
    tracing::event!(target: "sidevoice_shared::call", tracing::Level::ERROR, error.type = held, "the same type again, the same day");
    // Never, whatever the code says: the page's, the panic's two, a dependency's, and what is not an error.
    tracing::event!(target: "webview", tracing::Level::ERROR, error.type = failed, "the page's console");
    tracing::event!(target: "panic", tracing::Level::ERROR, error.type = failed, "the panic");
    tracing::event!(target: "crash_backtrace", tracing::Level::ERROR, error.type = failed, "the backtrace");
    tracing::event!(target: "reqwest::connect", tracing::Level::ERROR, error.type = failed, "a dependency");
    tracing::event!(target: "sidevoice_shared::call", tracing::Level::WARN, error.type = failed, "a warning");
    // A value that is not one of the app's codes, however well formed: counted, never sent.
    tracing::event!(target: "sidevoice_shared::call", tracing::Level::ERROR, error.type = "shared.not-one-of-ours", "a code nobody wrote");
    tracing::event!(target: "sidevoice_shared::call", tracing::Level::ERROR, error.type = "192.168.1.10", "an address");
    tracing::event!(
        target: "sidevoice_shared::call",
        tracing::Level::ERROR,
        error.type = failed,
        secret = "canary-secret-7f3a",
        path = "/home/canary-user-7f3a/x",
        "canary words 7f3a at /home/canary-user-7f3a"
    );
    wait_for("two events", || post.count() == 2);
    assert_eq!(post.types(), [held, failed]);
    assert_eq!(crash::unreported_errors(), 2);
    let body = String::from_utf8_lossy(&post.0.lock().unwrap()[1]).into_owned();
    for canary in ["canary-secret", "canary words", "canary-user", "/home/", "192.168", "\"message\"", "server_name"] {
        assert!(!body.contains(canary), "{canary} in {body}");
    }
    assert!(body.contains("canary-app@9.9.9"), "{body}");

    // Never: nothing is queued while it is on; back on, the next goes (and the one of the meantime is not among what was sent).
    reporter.set_mode(Mode::Never).unwrap();
    tracing::event!(target: "sidevoice_shared::call", tracing::Level::ERROR, error.type = SharedError::InvalidLogFilter.code(), "while never");
    reporter.set_mode(Mode::Auto).unwrap();
    tracing::event!(target: "sidevoice_shared::call", tracing::Level::ERROR, error.type = SharedError::CrashReporterFailed.code(), "after never");
    wait_for("the third event", || post.count() == 3);
    assert_eq!(post.types()[2], SharedError::CrashReporterFailed.code());
    assert_eq!(post.count(), 3);
}
