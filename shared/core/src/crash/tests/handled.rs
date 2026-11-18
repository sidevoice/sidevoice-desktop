//! Handled errors: what is read from an event, what is sent, how often, and never while the switch is off.

use tracing::{event, Level};
use tracing_subscriber::layer::SubscriberExt;

use super::*;
use crate::crash::handled::{day_of, event as handled_event, queueing, start as start_handling};
use crate::crash::handled::{Handled, Intake, Outcome, Worker};
use crate::crash::record::relative;

const SECRET: &str = "canary-secret-7f3a";
const WORDS: &str = "canary words 7f3a";
const MACHINE: &str = "0123456789abcdef0123456789abcdef";

// The app's own codes, the closed list its error enums give: only these leave.
const A: &str = "app.port-in-use";
const B: &str = "app.start-failed";
const C: &str = "app.already-running";
const OTHER: &str = "app.crashed";
const LIST: [&str; 4] = [A, B, C, OTHER];

/// The process's own targets, as the apps list them: ours, and the fixed ones.
const OWN: [&str; 4] = ["sidevoice_shared", "panic", "crash_backtrace", "webview"];

/// A reporter over `dir` that can send (to a recorder) under the default switch.
fn open(dir: &Path) -> (Arc<Reporter>, Arc<Recorder>) {
    let post = Arc::new(Recorder::default());
    (start(dir, &post), post)
}

/// What a subscriber with only the handled-errors layer, over `own` and `codes`, queues while `emit` logs, and how many events had
/// no code of the app's. The reporter answers the switch.
fn queued_in(
    own: &[&str],
    codes: &[&'static str],
    reporter: &Arc<Reporter>,
    capacity: usize,
    emit: impl FnOnce(),
) -> (Vec<Handled>, u64) {
    // Its own intake for this call (leaked: the layer wants a `'static` one), so that tests running side by side do not feed or
    // count each other's.
    let intake: &'static Intake = Box::leak(Box::new(Intake::new(capacity)));
    let waiting = intake.take_for_test(reporter.clone(), codes.to_vec());
    let subscriber = tracing_subscriber::registry().with(queueing(intake, own, || false));
    tracing::subscriber::with_default(subscriber, emit);
    (waiting.try_iter().collect(), intake.unreported.load(Ordering::Relaxed))
}

fn queued(reporter: &Arc<Reporter>, capacity: usize, emit: impl FnOnce()) -> (Vec<Handled>, u64) {
    queued_in(&OWN, &LIST, reporter, capacity, emit)
}

fn kinds(events: &[Handled]) -> Vec<&str> {
    events.iter().map(|event| event.kind).collect()
}

fn error_of(kind: &'static str) -> Handled {
    Handled { kind, file: "desktop/tauri/src/x.rs".to_owned(), line: 7 }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

/// What the day's file says, as the sender would read it at start.
fn load(reporter: &Reporter, today: u64) {
    reporter.handled.lock().unwrap().load(&LIST, today);
}

fn handle(worker: &mut Worker, reporter: &Reporter, error: &Handled, today: u64) -> Outcome {
    worker.handle(reporter, error, today)
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

const DAY: u64 = 20_400;

// Which events.

#[test]
fn only_error_events_of_our_own_targets_are_taken_and_never_the_panic_hooks_or_the_pages() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let (events, unreported) = queued(&reporter, 32, || {
        event!(target: "sidevoice_shared::call", Level::ERROR, error.type = A, "ours");
        event!(target: "sidevoice_shared", Level::ERROR, error.type = B, "the crate itself");
        // The panic hook's own, the page's, and whatever is below them: never, whatever the code says.
        event!(target: "panic", Level::ERROR, error.type = OTHER, "the panic");
        event!(target: "panic::deeper", Level::ERROR, error.type = OTHER, "below the panic");
        event!(target: "crash_backtrace", Level::ERROR, error.type = OTHER, "the backtrace");
        event!(target: "crash_backtrace::deeper", Level::ERROR, error.type = OTHER, "below it");
        event!(target: "webview", Level::ERROR, error.type = OTHER, "the page's console");
        event!(target: "webview::console", Level::ERROR, error.type = OTHER, "below the page's");
        // Not ours: a dependency, another piece, lookalikes that only share a prefix.
        event!(target: "reqwest::connect", Level::ERROR, error.type = OTHER, "a dependency");
        event!(target: "sidevoice_connector", Level::ERROR, error.type = OTHER, "another piece");
        event!(target: "sidevoice_sharedx", Level::ERROR, error.type = OTHER, "a lookalike");
        event!(target: "panic_x", Level::ERROR, error.type = OTHER, "a lookalike of the panic's");
        event!(target: "crash_backtrace_x", Level::ERROR, error.type = OTHER, "a lookalike of the backtrace's");
        event!(target: "webviewer", Level::ERROR, error.type = OTHER, "a lookalike of the page's");
        // Ours, but not an error.
        event!(target: "sidevoice_shared::x", Level::WARN, error.type = OTHER, "a warning");
        event!(target: "sidevoice_shared::x", Level::INFO, error.type = OTHER, "an info");
        event!(target: "sidevoice_shared::x", Level::DEBUG, error.type = OTHER, "a debug");
        event!(target: "sidevoice_shared::x", Level::TRACE, error.type = OTHER, "a trace");
    });
    assert_eq!(kinds(&events), [A, B]);
    assert_eq!(unreported, 0, "the filter comes before the count");
}

#[test]
fn a_target_below_the_pages_is_never_taken_even_when_the_process_lists_it_as_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let own = ["sidevoice_shared", "webview::console", "panic::deeper", "crash_backtrace::deeper"];
    let (events, unreported) = queued_in(&own, &LIST, &reporter, 16, || {
        event!(target: "webview::console", Level::ERROR, error.type = OTHER, "listed, and the page's");
        event!(target: "webview::console::x", Level::ERROR, error.type = OTHER, "below it");
        event!(target: "panic::deeper", Level::ERROR, error.type = OTHER, "listed, and the panic's");
        event!(target: "crash_backtrace::deeper", Level::ERROR, error.type = OTHER, "listed, and the backtrace's");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "ours");
    });
    assert_eq!(kinds(&events), [A]);
    assert_eq!(unreported, 0);
}

#[test]
fn an_event_is_read_for_its_code_and_its_place_and_for_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let line = line!() + 2;
    let (events, _) = queued(&reporter, 16, || {
        event!(
            target: "sidevoice_shared::call",
            Level::ERROR,
            error.type = "app.port-in-use",
            code.file.path = "/home/canary-user-7f3a/src/lie.rs",
            code.line.number = 99_999,
            secret = SECRET,
            path = HOME,
            words = %WORDS,
            detail = ?WORDS,
            kind = "canary",
            "the port canary words 7f3a was taken by canary-secret-7f3a"
        );
    });
    assert_eq!(
        events,
        [Handled {
            kind: "app.port-in-use",
            // The callsite's, not what a field claims.
            file: "shared/core/src/crash/tests/handled.rs".to_owned(),
            line,
        }]
    );
}

#[test]
fn an_event_without_a_code_of_the_apps_is_counted_and_never_taken() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let (events, unreported) = queued(&reporter, 32, || {
        event!(target: "sidevoice_shared::x", Level::ERROR, "no field at all");
        event!(target: "sidevoice_shared::x", Level::ERROR, reason = A, "another field");
        // The code only counts when it is a string.
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = ?A, "recorded with Debug");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = %A, "recorded with Display");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = 7, "a number");
        // A value of any other kind, well formed or not: a sentence, a path, an address, a host name, a word, a code of another
        // piece or one nobody wrote.
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "a sentence with spaces", "text");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = HOME, "a path");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "192.168.1.10", "an address");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "my-laptop.local", "a host name");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "app.alice-smith", "a name in the shape of a code");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "app.not-one-of-ours", "a shape-valid code nobody wrote");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "engine.provider-unreachable", "a dependency's code");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "", "empty");
        // Matched whole, nothing normalised.
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "App.Port-In-Use", "capitals");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "app.port-in-use ", "a space after");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "app.port-in-use\n", "a line after");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "app.port-in-use.x", "a longer one");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "the one that is");
    });
    assert_eq!(kinds(&events), [A]);
    assert_eq!(unreported, 17);
}

#[test]
fn every_code_the_shared_crate_names_is_a_code_that_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let codes: Vec<&'static str> = SharedError::codes().collect();
    let (events, unreported) = queued_in(&OWN, &codes, &reporter, 32, || {
        for code in SharedError::codes() {
            event!(target: "sidevoice_shared::x", Level::ERROR, error.type = code, "failed");
        }
    });
    assert_eq!(codes.len(), SharedError::ALL.len());
    assert_eq!(kinds(&events), codes);
    assert_eq!(unreported, 0);
    // The list of a build that names no code sends none.
    let (events, unreported) = queued_in(&OWN, &[], &reporter, 32, || {
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = SharedError::CrashReporterFailed.code(), "failed");
    });
    assert!(events.is_empty());
    assert_eq!(unreported, 1);
}

#[test]
fn the_place_is_a_short_relative_path_or_unknown() {
    assert_eq!(relative("desktop/tauri/src/error.rs"), "desktop/tauri/src/error.rs");
    assert_eq!(relative("shared\\core\\src\\fs.rs"), "shared/core/src/fs.rs", "a Windows build");
    for path in [
        "",
        "/home/canary-user-7f3a/.cargo/registry/x/a.rs",
        "C:\\Users\\canary-user-7f3a\\a.rs",
        "~/a.rs",
        "../a.rs",
        "a b.rs",
    ] {
        assert_eq!(relative(path), "unknown", "{path}");
    }
}

// The layer: the switch, the hook and the queue.

#[test]
fn nothing_is_queued_while_the_switch_is_never_or_cannot_be_read_or_the_build_cannot_send() {
    let emit = || event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "failed");
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    assert_eq!(queued(&reporter, 16, emit).0.len(), 1, "on by default");
    reporter.set_mode(Mode::Never).unwrap();
    assert!(queued(&reporter, 16, emit).0.is_empty(), "never");
    reporter.set_mode(Mode::Auto).unwrap();
    assert_eq!(queued(&reporter, 16, emit).0.len(), 1, "on again");

    // A setting that cannot be read is never for that run: nothing is queued, and the file is left as it is.
    let broken = tempfile::tempdir().unwrap();
    std::fs::write(broken.path().join(settings::FILE), b"not json").unwrap();
    let (unknown, _) = open(broken.path());
    assert!(unknown.mode() == Mode::Never);
    assert!(queued(&unknown, 16, emit).0.is_empty(), "unknown");
    assert_eq!(std::fs::read(broken.path().join(settings::FILE)).unwrap(), b"not json");

    // A build with nowhere to send to, and a reporter that could not start.
    let dev = tempfile::tempdir().unwrap();
    let nowhere = Reporter::start(config(dev.path(), None));
    assert!(queued(&nowhere, 16, emit).0.is_empty(), "no DSN");
    assert!(queued(&Reporter::unavailable(resource()), 16, emit).0.is_empty(), "unavailable");
}

#[test]
fn a_reporter_thread_and_the_panic_hook_never_feed_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let emit = || event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "failed");
    SENDING.set(true);
    let from_a_sender = queued(&reporter, 16, emit).0;
    SENDING.set(false);
    assert!(from_a_sender.is_empty());
    let intake: &'static Intake = Box::leak(Box::new(Intake::new(16)));
    let waiting = intake.take_for_test(reporter.clone(), LIST.to_vec());
    let subscriber = tracing_subscriber::registry().with(queueing(intake, &OWN, || true));
    tracing::subscriber::with_default(subscriber, emit);
    assert!(waiting.try_iter().next().is_none(), "inside the hook");
}

#[test]
fn a_full_queue_drops_the_event_and_the_logging_thread_never_waits() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let (events, _) = queued(&reporter, 2, || {
        for n in 0..50 {
            event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, n, "failed");
        }
    });
    assert_eq!(events.len(), 2);
}

// The event.

#[test]
fn the_event_carries_the_closed_field_list_and_no_text_of_the_event() {
    std::env::set_var("HOSTNAME", "canary-host-7f3a");
    std::env::set_var("COMPUTERNAME", "canary-host-7f3a");
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = open(dir.path());
    let (events, _) = queued(&reporter, 16, || {
        event!(
            target: "sidevoice_shared::call",
            Level::ERROR,
            error.type = "app.port-in-use",
            secret = SECRET,
            path = HOME,
            host = "canary-host-7f3a",
            words = %WORDS,
            "the port canary words 7f3a was taken by canary-secret-7f3a at /home/canary-user-7f3a on canary-host-7f3a"
        );
    });
    let value = serde_json::to_value(handled_event(&resource(), &events[0], MACHINE)).unwrap();
    assert_eq!(
        keys(&value),
        ["event_id", "exception", "platform", "release", "tags", "timestamp"],
        "no extra, breadcrumb, context, user, request, message or host name, and the default level: {value}"
    );
    // Sentry's default level is error, so the SDK's type leaves it out: the field is there only when it is not the default.
    assert!(value.get("level").is_none(), "{value}");
    assert_eq!(value["release"], "sidevoice-desktop@1.2.3");
    assert_eq!(keys(&value["tags"]), {
        let mut tags = report::TAGS.to_vec();
        tags.sort_unstable();
        tags
    });
    assert_eq!(value["tags"]["service.name"], "sidevoice-desktop");
    assert_eq!(value["tags"]["os.type"], "linux");
    assert_eq!(value["tags"]["os.name"], "Linux");
    assert_eq!(value["tags"]["host.arch"], "amd64");
    assert_eq!(value["tags"]["installation.id"], MACHINE);
    let exception = &value["exception"]["values"];
    assert_eq!(exception.as_array().unwrap().len(), 1);
    assert_eq!(keys(&exception[0]), ["stacktrace", "type"], "no message");
    assert_eq!(exception[0]["type"], "app.port-in-use");
    let frames = exception[0]["stacktrace"]["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(keys(&frames[0]), ["filename", "lineno"]);
    assert_eq!(frames[0]["filename"], "shared/core/src/crash/tests/handled.rs");
    assert!(frames[0]["lineno"].as_u64().unwrap() > 0);
    // Random per event: nothing links two of them but the install id.
    let other = serde_json::to_value(handled_event(&resource(), &events[0], MACHINE)).unwrap();
    assert_ne!(value["event_id"], other["event_id"]);

    let json = value.to_string();
    for canary in ["canary", "/home/"] {
        assert!(!json.contains(canary), "{canary} in {json}");
    }
}

#[test]
fn a_place_that_is_not_ours_to_say_is_unknown_in_the_event() {
    let place = Handled { file: relative("/home/canary-user-7f3a/.cargo/x/a.rs"), ..error_of(A) };
    let json = serde_json::to_string(&handled_event(&resource(), &place, MACHINE)).unwrap();
    assert!(json.contains("\"filename\":\"unknown\""), "{json}");
    assert!(!json.contains("canary") && !json.contains("/home/"), "{json}");
}

#[test]
fn the_day_is_the_utc_day_since_the_epoch() {
    assert_eq!(day_of(UNIX_EPOCH), 0);
    assert_eq!(day_of(UNIX_EPOCH + Duration::from_secs(24 * 60 * 60 - 1)), 0);
    assert_eq!(day_of(UNIX_EPOCH + Duration::from_secs(24 * 60 * 60 * 3 + 5)), 3);
    assert_eq!(day_of(UNIX_EPOCH - Duration::from_secs(5)), 0, "a clock before the epoch is day 0, not a panic");
}

// The sender.

#[test]
fn an_error_is_sent_once_a_day_per_type_and_a_new_day_or_a_clock_gone_back_is_a_new_day() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, post) = open(dir.path());
    let mut worker = Worker::default();
    let (day, next, before) = (DAY, DAY + 1, DAY - 1);

    assert_eq!(handle(&mut worker, &reporter, &error_of(A), day), Outcome::Sent);
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), day), Outcome::Capped);
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), day), Outcome::Capped);
    assert_eq!(handle(&mut worker, &reporter, &error_of(B), day), Outcome::Sent);
    assert_eq!(post.count(), 2, "one per type");

    assert_eq!(handle(&mut worker, &reporter, &error_of(A), next), Outcome::Sent, "tomorrow");
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), next), Outcome::Capped);
    assert_eq!(post.count(), 3);

    // The clock went back to a day the list is not for: a new day, and the list is that day's from now on.
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), before), Outcome::Sent, "clock back");
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), before), Outcome::Capped);
    assert_eq!(handle(&mut worker, &reporter, &error_of(B), before), Outcome::Sent);
    assert_eq!(post.count(), 5);
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), day), Outcome::Sent, "and forward again");
    assert_eq!(post.count(), 6);
}

#[test]
fn the_days_list_is_a_file_beside_the_setting_that_survives_a_restart_for_that_day_only() {
    let dir = tempfile::tempdir().unwrap();
    {
        let (reporter, _) = open(dir.path());
        let mut worker = Worker::default();
        assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Sent);
        assert_eq!(handle(&mut worker, &reporter, &error_of(B), DAY), Outcome::Sent);
    }
    let file = dir.path().join("handled-errors.json");
    assert_eq!(names(dir.path()), ["crash-reports.json", "handled-errors.json"]);
    let value: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(keys(&value), ["day", "sent"], "codes and the day, nothing else");
    assert_eq!(value["day"], DAY);
    assert_eq!(value["sent"], serde_json::json!([A, B]));

    // Another run, the same day: the list is read back.
    let (reporter, post) = open(dir.path());
    load(&reporter, DAY);
    let mut worker = Worker::default();
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Capped);
    assert_eq!(handle(&mut worker, &reporter, &error_of(C), DAY), Outcome::Sent);
    assert_eq!(post.count(), 1);

    // A file for another day (earlier or later), a file that is not ours, and one that is not a list: an empty list.
    for kept in [
        serde_json::json!({"day": DAY - 1, "sent": [A]}),
        serde_json::json!({"day": DAY + 1, "sent": [A]}),
        serde_json::json!({"day": "not a day", "sent": [A]}),
        serde_json::json!([A]),
    ] {
        std::fs::write(&file, kept.to_string()).unwrap();
        let (fresh, _) = open(dir.path());
        load(&fresh, DAY);
        assert_eq!(handle(&mut Worker::default(), &fresh, &error_of(A), DAY), Outcome::Sent, "{kept}");
    }
    std::fs::write(&file, b"not json").unwrap();
    let (fresh, _) = open(dir.path());
    load(&fresh, DAY);
    assert_eq!(handle(&mut Worker::default(), &fresh, &error_of(A), DAY), Outcome::Sent);
    let value: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(value["sent"], serde_json::json!([A]), "replaced by a good one");
}

#[test]
fn a_file_with_things_that_are_not_the_apps_codes_is_read_as_its_codes_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("handled-errors.json"),
        serde_json::json!({"day": DAY, "sent": [
            "not a code", HOME, A, A, "App.Start-Failed", 7, "app.not-one-of-ours", "192.168.1.10", C
        ]})
        .to_string(),
    )
    .unwrap();
    let (reporter, _) = open(dir.path());
    load(&reporter, DAY);
    assert_eq!(reporter.handled.lock().unwrap().sent(), [A, C], "the codes, once each");
    let mut worker = Worker::default();
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Capped);
    assert_eq!(handle(&mut worker, &reporter, &error_of(C), DAY), Outcome::Capped);
    assert_eq!(
        handle(&mut worker, &reporter, &error_of(B), DAY),
        Outcome::Sent,
        "what is not one of the codes says nothing about it"
    );
    let value: Value = serde_json::from_slice(&std::fs::read(dir.path().join("handled-errors.json")).unwrap()).unwrap();
    assert_eq!(value["sent"], serde_json::json!([A, C, B]));
}

#[test]
fn the_days_sends_are_bounded_by_a_closed_list_of_at_most_max_codes_and_its_file_stays_readable() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, post) = open(dir.path());
    // More codes than the cap allows, with repeats: the sender takes the first MAX_CODES once each.
    let made: Vec<&'static str> = (0..MAX_CODES + 20)
        .map(|n| &*Box::leak(format!("app.made-up-code-for-a-long-name-{n:03}").into_boxed_str()))
        .collect();
    let intake: &'static Intake = Box::leak(Box::new(Intake::new(4)));
    let listed: Vec<&'static str> = made.iter().chain(made.iter()).copied().collect();
    start_handling(intake, reporter.clone(), listed);
    assert_eq!(intake.codes().unwrap(), &made[..MAX_CODES]);
    assert!(SharedError::ALL.len() <= MAX_CODES);

    // Every one of them goes out once in a day, the most a day sends, and the file that says so is still read back.
    let mut worker = Worker::default();
    for code in &made[..MAX_CODES] {
        assert_eq!(handle(&mut worker, &reporter, &error_of(code), DAY), Outcome::Sent, "{code}");
    }
    assert_eq!(post.count(), MAX_CODES);
    let size = std::fs::metadata(dir.path().join("handled-errors.json")).unwrap().len();
    assert!(size < 4 * 1024, "{size} bytes: the file must be read back");
    let (again, _) = open(dir.path());
    again.handled.lock().unwrap().load(&made[..MAX_CODES], DAY);
    assert_eq!(again.handled.lock().unwrap().sent(), &made[..MAX_CODES]);
}

#[test]
fn nothing_is_sent_or_made_while_the_switch_is_never_or_cannot_be_read_or_the_build_cannot_send() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, post) = open(dir.path());
    reporter.set_mode(Mode::Never).unwrap();
    let mut worker = Worker::default();
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Off);
    assert_eq!(post.count(), 0);
    assert_eq!(names(dir.path()), ["crash-reports.json"], "no id and no list were made");

    // A setting that cannot be read.
    let broken = tempfile::tempdir().unwrap();
    std::fs::write(broken.path().join(settings::FILE), b"{\"mode\":\"sometimes\"}").unwrap();
    let (unknown, post) = open(broken.path());
    assert_eq!(handle(&mut worker, &unknown, &error_of(A), DAY), Outcome::Off);
    assert_eq!(post.count(), 0);
    assert_eq!(names(broken.path()), ["crash-reports.json"], "the file is left as it is, nothing else made");

    // A build with nowhere to send to.
    let dev = tempfile::tempdir().unwrap();
    let nowhere = Reporter::start(config(dev.path(), None));
    assert_eq!(handle(&mut worker, &nowhere, &error_of(A), DAY), Outcome::Off);
    assert!(names(dev.path()).is_empty());
}

#[test]
fn never_erases_the_days_list_with_the_install_id_and_what_the_sender_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, post) = open(dir.path());
    let mut worker = Worker::default();
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Sent);
    let id = reporter.installation_id().unwrap();
    assert_eq!(names(dir.path()), ["crash-reports.json", "handled-errors.json"]);

    reporter.set_mode(Mode::Never).unwrap();
    assert_eq!(names(dir.path()), ["crash-reports.json"], "the list is gone with the records");
    let text = std::fs::read_to_string(dir.path().join(settings::FILE)).unwrap();
    assert!(!text.contains(&id), "{text}");
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Off);

    // Back on: what was forgotten is not remembered, so the type goes again, under a new id.
    reporter.set_mode(Mode::Auto).unwrap();
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Sent);
    assert_eq!(post.count(), 2);
    assert_ne!(reporter.installation_id().unwrap(), id);

    // A start under never erases it as well (the file may have come back with a restore).
    reporter.set_mode(Mode::Never).unwrap();
    std::fs::write(dir.path().join("handled-errors.json"), b"{}").unwrap();
    let (restarted, _) = open(dir.path());
    assert!(restarted.wait_idle(Duration::from_secs(10)));
    assert_eq!(names(dir.path()), ["crash-reports.json"]);
}

/// A transport that turns the switch off while the POST is on the wire, and keeps the id the event went out under.
struct TurnsItOff {
    reporter: Mutex<Option<Arc<Reporter>>>,
    id: Mutex<Option<String>>,
}

impl Post for TurnsItOff {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        let reporter = self.reporter.lock().unwrap().clone().expect("the reporter is set");
        *self.id.lock().unwrap() = reporter.installation_id();
        reporter.set_mode(Mode::Never).unwrap();
        Ok(())
    }
}

#[test]
fn a_switch_turned_off_while_the_post_is_on_the_wire_leaves_no_list_behind() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(TurnsItOff { reporter: Mutex::new(None), id: Mutex::new(None) });
    let reporter = Reporter::start(config(dir.path(), Some(post.clone())));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    *post.reporter.lock().unwrap() = Some(reporter.clone());
    assert_eq!(handle(&mut Worker::default(), &reporter, &error_of(A), DAY), Outcome::Sent);
    assert_eq!(names(dir.path()), ["crash-reports.json"], "no list for an app set not to send");
    let id = post.id.lock().unwrap().clone().expect("the event went out under an id");
    let text = std::fs::read_to_string(dir.path().join(settings::FILE)).unwrap();
    assert!(text.contains("never") && !text.contains(&id), "{text}");
    *post.reporter.lock().unwrap() = None;
}

#[test]
fn a_failed_send_is_not_counted_and_the_network_is_left_alone_for_a_while() {
    let dir = tempfile::tempdir().unwrap();
    let refuses = Arc::new(Refuses(AtomicUsize::new(0)));
    let reporter = Reporter::start(config(dir.path(), Some(refuses.clone())));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    let mut worker = Worker::default();
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Unsent);
    assert_eq!(handle(&mut worker, &reporter, &error_of(B), DAY), Outcome::Quiet);
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Quiet);
    assert_eq!(refuses.0.load(Ordering::SeqCst), 1, "one try, then quiet");
    assert!(!dir.path().join("handled-errors.json").exists(), "nothing was sent");

    // The same type, on a later run with the network back: it was never counted.
    let (later, post) = open(dir.path());
    assert_eq!(handle(&mut Worker::default(), &later, &error_of(A), DAY), Outcome::Sent);
    assert_eq!(post.count(), 1);
}

/// A transport that breaks on its first call and records the others.
#[derive(Default)]
struct BreaksOnce {
    calls: AtomicUsize,
    then: Recorder,
}

impl Post for BreaksOnce {
    fn post(&self, envelope: Vec<u8>, budget: Duration) -> Result<(), PostError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("the transport broke");
        }
        self.then.post(envelope, budget)
    }
}

#[test]
fn an_install_id_that_cannot_be_made_is_one_line_and_a_quiet_spell_and_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the setting's temporary file goes: the id cannot be written.
    std::fs::create_dir(dir.path().join("crash-reports.json.tmp")).unwrap();
    let (reporter, post) = open(dir.path());
    assert!(reporter.is_open());
    let mut worker = Worker::default();
    assert_eq!(handle(&mut worker, &reporter, &error_of(A), DAY), Outcome::Unsent);
    assert_eq!(handle(&mut worker, &reporter, &error_of(B), DAY), Outcome::Quiet);
    assert_eq!(post.count(), 0);
    assert!(!dir.path().join("handled-errors.json").exists());
}

#[test]
fn a_transport_that_panics_costs_that_event_and_not_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(BreaksOnce::default());
    let reporter = Reporter::start(config(dir.path(), Some(post.clone())));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    let intake: &'static Intake = Box::leak(Box::new(Intake::new(4)));
    start_handling(intake, reporter, LIST.to_vec());
    let subscriber = tracing_subscriber::registry().with(queueing(intake, &OWN, || false));
    tracing::subscriber::with_default(subscriber, || {
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "the one that breaks the transport");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = B, "the next");
        wait_for("the next event", || post.then.count() == 1);
        assert_eq!(post.then.event(0)["exception"]["values"][0]["type"], B);
        // The type whose send panicked was not counted as sent.
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "again");
        wait_for("that type again", || post.then.count() == 2);
        assert_eq!(post.then.event(1)["exception"]["values"][0]["type"], A);
    });
}

#[test]
fn what_reaches_the_transport_is_the_closed_event_under_the_install_id() {
    std::env::set_var("HOSTNAME", "canary-host-7f3a");
    let dir = tempfile::tempdir().unwrap();
    let (reporter, post) = open(dir.path());
    let (events, _) = queued(&reporter, 16, || {
        event!(
            target: "sidevoice_shared::call",
            Level::ERROR,
            error.type = "app.port-in-use",
            secret = SECRET,
            path = HOME,
            words = %WORDS,
            "the port canary words 7f3a was taken by canary-secret-7f3a at /home/canary-user-7f3a on canary-host-7f3a"
        );
    });
    let mut worker = Worker::default();
    assert_eq!(handle(&mut worker, &reporter, &events[0], DAY), Outcome::Sent);
    assert_eq!(post.count(), 1);
    let body = String::from_utf8_lossy(&post.0.lock().unwrap()[0]).into_owned();
    assert!(body.contains("app.port-in-use"), "{body}");
    // The install id is the one the crash reports use, and the page shows.
    assert!(body.contains(&reporter.installation_id().unwrap()), "{body}");
    for canary in ["canary", "/home/", "breadcrumbs", "server_name", "\"user\"", "\"request\"", "\"extra\""] {
        assert!(!body.contains(canary), "{canary} in {body}");
    }
    let event = post.event(0);
    assert_eq!(event["release"], "sidevoice-desktop@1.2.3");
    assert_eq!(event["exception"]["values"][0]["type"], "app.port-in-use");
}

#[test]
fn a_well_formed_code_that_is_not_the_apps_never_reaches_the_transport() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, post) = open(dir.path());
    let (events, unreported) = queued(&reporter, 16, || {
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "192.168.1.10", "an address");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "my-laptop.local", "a host name");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = "app.not-one-of-ours", "a code nobody wrote");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "a code of the app's");
    });
    assert_eq!(unreported, 3);
    let mut worker = Worker::default();
    for error in &events {
        assert_eq!(handle(&mut worker, &reporter, error, DAY), Outcome::Sent);
    }
    assert_eq!(post.count(), 1, "only the code of the app's");
    let body = String::from_utf8_lossy(&post.0.lock().unwrap()[0]).into_owned();
    assert!(body.contains(A), "{body}");
    for other in ["192.168.1.10", "my-laptop.local", "not-one-of-ours"] {
        assert!(!body.contains(other), "{other} in {body}");
    }
}

// From the layer to the transport, on the sender's own thread.

#[test]
fn from_an_event_to_the_transport_once_a_type_a_day_and_never_under_never() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, post) = open(dir.path());
    let intake: &'static Intake = Box::leak(Box::new(Intake::new(16)));
    assert!(intake.codes().is_none());
    start_handling(intake, reporter.clone(), LIST.to_vec());
    assert_eq!(intake.codes().unwrap(), LIST);
    let subscriber = tracing_subscriber::registry().with(queueing(intake, &OWN, || false));
    tracing::subscriber::with_default(subscriber, || {
        // The sender takes them in order: the second A is capped before B is sent.
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "failed");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = A, "failed again");
        event!(target: "webview", Level::ERROR, error.type = OTHER, "the page's");
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = B, "failed too");
        wait_for("two events", || post.count() == 2);
        assert_eq!(post.event(0)["exception"]["values"][0]["type"], A);
        assert_eq!(post.event(1)["exception"]["values"][0]["type"], B);

        // Under never nothing is queued at all; back on, the next one goes (and C is not among what was sent).
        reporter.set_mode(Mode::Never).unwrap();
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = C, "failed while never");
        reporter.set_mode(Mode::Auto).unwrap();
        event!(target: "sidevoice_shared::x", Level::ERROR, error.type = OTHER, "failed after never");
        wait_for("a third event", || post.count() == 3);
        assert_eq!(post.event(2)["exception"]["values"][0]["type"], OTHER);
    });
    assert_eq!(post.count(), 3);
}

#[test]
fn a_reporter_that_cannot_send_watches_for_nothing_and_a_second_start_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let intake: &'static Intake = Box::leak(Box::new(Intake::new(16)));
    start_handling(intake, Reporter::start(config(dir.path(), None)), LIST.to_vec());
    assert!(intake.codes().is_none(), "no DSN: the layer is never bound");
    start_handling(intake, Reporter::unavailable(resource()), LIST.to_vec());
    assert!(intake.codes().is_none());

    let (reporter, _) = open(dir.path());
    start_handling(intake, reporter.clone(), vec![A]);
    assert_eq!(intake.codes().unwrap(), [A]);
    start_handling(intake, reporter, LIST.to_vec());
    assert_eq!(intake.codes().unwrap(), [A], "the first start stands");
}
