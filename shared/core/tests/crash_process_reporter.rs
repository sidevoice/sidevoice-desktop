//! The reporter is the process's, started once. The hooks it binds to (the panic hook's observer, the handled errors' layer) are the
//! process's and bind to the first reporter only; the phone's app runs its setup again when Android recreates the activity, and a
//! second reporter would hold its own copy of the setting: a "No enviar" chosen on it left the first one sending, and the first,
//! making its install id from its own copy, put `auto` back over the choice. Its own test binary: the hooks are the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::crash::{self, Config, Mode, Post, PostError};
use sidevoice_shared::error::SharedError;
use sidevoice_shared::logging::{self, Resource};
use std::path::Path;
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
}

fn config(dir: &Path, post: &Arc<Recorder>) -> Config {
    Config {
        resource: Resource::of("canary-app", "9.9.9", "linux", "x86_64"),
        dsn: None,
        dir: dir.to_owned(),
        post: Some(post.clone()),
    }
}

fn records(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("crash-records")).map(|entries| entries.flatten().count()).unwrap_or(0)
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
fn a_second_setup_gets_the_first_reporter_and_no_enviar_holds_for_every_hook() {
    let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let own = ["sidevoice_shared", logging::PANIC_TARGET, logging::BACKTRACE_TARGET, logging::WEBVIEW_TARGET];
    logging::init(&own, None, |_| {});
    assert!(crash::process_reporter().is_none(), "nothing started yet");
    let (post_of_first, post_of_second) = (Arc::new(Recorder::default()), Arc::new(Recorder::default()));

    let before = crash::start_process_reporter(config(first.path(), &post_of_first), SharedError::codes());
    assert!(before.wait_idle(Duration::from_secs(10)));
    // Android recreates the activity: the app's setup runs again, with a folder and a transport of its own.
    let after = crash::start_process_reporter(config(second.path(), &post_of_second), SharedError::codes());
    assert!(Arc::ptr_eq(&before, &after), "a second start is the first reporter");
    assert!(Arc::ptr_eq(&before, &crash::process_reporter().unwrap()));

    // The hooks are bound to it: a handled error goes out through its transport (never the second's), a panic leaves its record.
    let id = after.installation_id().expect("an id under auto");
    tracing::event!(
        target: "sidevoice_shared::call",
        tracing::Level::ERROR,
        error.type = SharedError::VoiceModelsHeld.code(),
        "before no enviar"
    );
    wait_for("the handled error", || post_of_first.count() == 1);
    assert!(std::thread::spawn(|| panic!("a panic before no enviar")).join().is_err());
    assert_eq!(records(first.path()), 1, "the panic hook records through the reporter");

    // "No enviar", chosen on the reporter the app's state of the second setup manages.
    after.set_mode(Mode::Never).unwrap();
    assert_eq!(records(first.path()), 0, "the record is erased");
    tracing::event!(
        target: "sidevoice_shared::call",
        tracing::Level::ERROR,
        error.type = SharedError::NativeEngineFailed.code(),
        "after no enviar"
    );
    assert!(std::thread::spawn(|| panic!("a panic after no enviar")).join().is_err());
    // Nothing more is queued, sent or recorded, whichever hook it came through (the wait only makes a send that should not be
    // there show up).
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(post_of_first.count(), 1, "nothing sent after no enviar");
    assert_eq!(post_of_second.count(), 0, "the second transport was never the process's");
    assert_eq!(records(first.path()), 0, "no record after no enviar");

    // The install id cannot come back over the choice: not from the first reporter's copy, not from the second's.
    assert_eq!(before.installation_id(), None);
    assert_eq!(after.installation_id(), None);
    let setting = std::fs::read_to_string(first.path().join("crash-reports.json")).unwrap();
    assert!(setting.contains("\"never\"") && !setting.contains(&id), "{setting}");
    assert!(std::fs::read_dir(second.path()).unwrap().next().is_none(), "the second folder was never written");
}
