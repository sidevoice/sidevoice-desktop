//! The canary test of the logging guideline (section 10) for the shared subscriber: its own test binary, one test and the
//! global subscriber, so every thread and the `log` bridge are captured. Seeds a home directory, words and a credential URL in
//! each way a line can reach the log, then asserts none shows, with positive controls so it cannot pass by capturing nothing.

use std::sync::{Arc, Mutex};

const HOME: &str = "/home/canary-user-7f3a";
const WORDS: &str = "canary words 7f3a";
const URL: &str = "https://u:canary-pass@x/?canary-q=1";

#[test]
fn nothing_seeded_reaches_the_log() {
    std::env::set_var("SIDEVOICE_LOG", "trace");
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&lines);
    sidevoice_shared::logging::init(&["sidevoice_shared"], Some(std::path::Path::new(HOME)), move |line| {
        sink.lock().unwrap().push(line.to_owned())
    });

    // Our own target: a path the code was careless with, on this thread and on another.
    tracing::info!(target: "sidevoice_shared::canary", stage = "open", path = %format!("{HOME}/project"), "canary event");
    std::thread::spawn(|| tracing::debug!(target: "sidevoice_shared::canary", stage = "thread", path = %format!("{HOME}/other"), "canary thread")).join().unwrap();
    // The `log` bridge: our own target with the path, and third parties that must stay out at any level.
    log::warn!(target: "sidevoice_shared::canary", "canary bridge {HOME}/log");
    log::info!(target: "tungstenite::handshake", "Request: {URL} {WORDS}");
    tracing::debug!(target: "reqwest::connect", "{URL} {WORDS}");
    // The SIDEVOICE_DEBUG helpers are not the log: a voice event with a transcript goes through them in the call host.
    sidevoice_shared::debug::debug(&format!("voice event {{\"phase\":\"finished\",\"text\":\"{WORDS}\"}}"));
    sidevoice_shared::debug::debug_detail(|| format!("snapshot {WORDS}"));
    // A panic that carries the path, on another thread and caught on this one.
    let _ = std::thread::Builder::new()
        .name("canary-thread".into())
        .spawn(|| panic!("failed at {HOME}/panic"))
        .unwrap()
        .join();
    let _ = std::panic::catch_unwind(|| panic!("second {HOME}/caught"));

    // The hook swallows panics; a failed assertion below must say why.
    drop(std::panic::take_hook());
    let text = lines.lock().unwrap().join("\n");
    for expected in [
        "canary event",
        "stage=\"open\"",
        "path=~/project",
        "canary thread",
        "canary bridge ~/log",
        "ERROR panic: panic",
        "canary-thread",
        "~/panic",
        "~/caught",
    ] {
        let found = text.contains(expected);
        assert!(found, "missing {expected:?} in:\n{text}");
    }
    let decimal = |seed: &str| seed.bytes().map(|b| b.to_string()).collect::<Vec<_>>().join(", ");
    for seed in ["canary-user-7f3a", WORDS, "canary-pass", "canary-q", "Request:"] {
        assert!(!text.contains(seed), "{seed:?} reached the log:\n{text}");
        assert!(!text.contains(&decimal(seed)), "{seed:?} reached the log as bytes");
    }
    assert!(!text.contains(HOME), "the home directory reached the log:\n{text}");
}
