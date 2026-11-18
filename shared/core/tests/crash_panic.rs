//! A panic reaches the crash reporter through the log's panic hook (sidevoice-desktop#523): the hook leaves a record and sends
//! nothing; the next start sends it. A literal message goes as it is, a formatted one only as the marker, and nothing of its
//! text, of a home path or of anything outside the closed list is in the record or in what is sent. Its own test binary and one
//! test: the hook and the subscriber are the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::crash::{self, Config, Mode, Post, PostError, Reporter};
use sidevoice_shared::logging::{self, Resource};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Post for Recorder {
    fn post(&self, envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        self.0.lock().unwrap().push(String::from_utf8_lossy(&envelope).into_owned());
        Ok(())
    }
}

fn resource() -> Resource {
    Resource::of("canary-app", "9.9.9", "linux", "x86_64")
}

fn start(dir: &Path, post: &Arc<Recorder>) -> Arc<Reporter> {
    let reporter =
        Reporter::start(Config { resource: resource(), dsn: None, dir: dir.to_owned(), post: Some(post.clone()) });
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    reporter
}

fn records(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("crash-records"))
        .map(|entries| {
            entries.flatten().map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default()).collect()
        })
        .unwrap_or_default()
}

fn event(envelope: &str) -> serde_json::Value {
    envelope
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value.get("exception").is_some())
        .expect("an event")
}

#[test]
fn a_panic_leaves_a_record_and_the_next_start_sends_it_as_a_literal_or_the_marker_and_never_as_its_formatted_text() {
    let dir = tempfile::tempdir().unwrap();
    logging::init(
        &["sidevoice_shared", logging::PANIC_TARGET, logging::BACKTRACE_TARGET],
        Some(Path::new("/home/canary-user-7f3a")),
        |_| {},
    );
    logging::announce(resource());
    let first = Arc::new(Recorder::default());
    let reporter = start(dir.path(), &first);
    crash::report_panics(reporter.clone());

    // What the app said before it failed: ours, and a dependency's (which the tail never keeps).
    tracing::warn!(
        target: "sidevoice_shared::engine",
        device = "d-77",
        path = "/home/canary-user-7f3a/models",
        "the model could not be loaded"
    );
    tracing::error!(target: "reqwest::connect", "a dependency with /home/canary-user-7f3a/x");

    let literal = std::thread::Builder::new()
        .name("literal-panic".into())
        .spawn(|| panic!("literal words that may be sent"))
        .unwrap();
    assert!(literal.join().is_err());
    let started = Instant::now();
    let formatted = std::thread::Builder::new()
        .name("formatted-panic".into())
        .spawn(|| {
            let result: Result<(), String> =
                std::hint::black_box(Err("/home/canary-user-7f3a/canary-secret-7f3a".into()));
            result.unwrap();
        })
        .unwrap();
    assert!(formatted.join().is_err());
    assert!(started.elapsed() < Duration::from_secs(5), "the hook is held up for a bounded time");

    // Nothing is sent at panic time; each panic left a record, with nothing of the formatted text or the home.
    assert!(first.0.lock().unwrap().is_empty(), "nothing is sent from the hook");
    let kept = records(dir.path());
    assert_eq!(kept.len(), 2, "{kept:?}");
    for text in &kept {
        assert!(!text.contains("canary-secret") && !text.contains("canary-user"), "{text}");
    }

    // The next start sends them, once.
    let second = Arc::new(Recorder::default());
    start(dir.path(), &second);
    let sent = second.0.lock().unwrap().clone();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(records(dir.path()).is_empty(), "deleted after a successful send");
    let events: Vec<serde_json::Value> = sent.iter().map(|envelope| event(envelope)).collect();
    let value = |event: &serde_json::Value| event["exception"]["values"][0]["value"].as_str().unwrap().to_owned();
    let literal = events.iter().find(|event| value(event) == "literal words that may be sent").expect("the literal");
    let formatted =
        events.iter().find(|event| value(event) == "<formatted panic message omitted>").expect("the marker");
    assert!(
        formatted["extra"]["len"].as_u64().unwrap() > 0,
        "the marker comes with the length of the formatted message"
    );
    for envelope in &sent {
        assert!(!envelope.contains("canary-secret") && !envelope.contains("canary-user"), "{envelope}");
        assert!(envelope.contains("\"release\":\"canary-app@9.9.9\""), "{envelope}");
        assert!(!envelope.contains("server_name") && !envelope.contains("\"user\""), "{envelope}");
    }
    for event in [literal, formatted] {
        let frames = event["exception"]["values"][0]["stacktrace"]["frames"].as_array().unwrap();
        assert!(!frames.is_empty() && frames.iter().all(|frame| frame["instruction_addr"].is_string()));
        let names: Vec<&str> = frames.iter().filter_map(|frame| frame["function"].as_str()).collect();
        // The panic hook swallows an assertion's message, so a stack that is refused would say nothing: say it here (a failed test's
        // output is printed).
        println!("the frames of the event for {:?}: {names:#?}", value(event));
        assert!(
            names.iter().any(|name| name.contains("crash_panic")),
            "the code that panicked is in the stack: {names:?}"
        );
        // A function is its path: Windows names carry the type arguments (`next<..sidevoice_shared::crash::handled::..>`), and a
        // frame the debug info has no symbol for (the unwinder's `__rust_try` shim, between `catch_unwind` and its `do_call`) is
        // named after whichever function precedes it in the image, whose arguments may name anything of ours. What must not be in
        // the stack is the panic machinery and the hook as functions, not as a word in the arguments of some other.
        let path = |name: &str| name.split('<').next().unwrap_or(name).to_owned();
        assert!(
            !names.iter().map(|name| path(name)).any(|name| {
                name.contains("rust_begin_unwind")
                    || name.starts_with("core::panicking::")
                    || name.starts_with("std::panicking::rust_panic_with_hook")
                    || name.starts_with("std::panicking::begin_panic_handler")
                    || name.contains("sidevoice_shared::logging")
                    || name.contains("sidevoice_shared::crash")
            }),
            "the panic machinery and the hook are not: {names:?}"
        );
        // The last warnings of our own targets, the path as `~`, the id a token; the dependency's line is not there.
        let tail: Vec<&str> =
            event["extra"]["log_tail"].as_array().unwrap().iter().filter_map(|line| line.as_str()).collect();
        assert_eq!(tail.len(), 1, "{tail:?}");
        assert!(tail[0].contains("WARN sidevoice_shared::engine: the model could not be loaded"), "{}", tail[0]);
        assert!(tail[0].contains("device=d1") && tail[0].contains("path=\"~/models\""), "{}", tail[0]);
        assert_eq!(event["tags"]["service.name"], "canary-app");
        assert_eq!(event["tags"]["os.type"], "linux");
        assert_eq!(event["tags"]["host.arch"], "amd64");
    }

    // Under "never", the next panic leaves nothing.
    reporter.set_mode(Mode::Never).unwrap();
    let after = std::thread::spawn(|| panic!("after never"));
    assert!(after.join().is_err());
    assert!(records(dir.path()).is_empty());
    assert_eq!(second.0.lock().unwrap().len(), 2);
}
