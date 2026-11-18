//! The reporter's behaviour end to end, with a transport of the test's own and a directory of its own. What needs the process's
//! panic hook is in `tests/crash_panic*.rs` (a binary each: the hook and the subscriber are the process's).

use super::record::{CrashRecord, Frame, Image, Panic};
use super::report::{self, EXTRAS, TAGS};
use super::store::{Store, MAX_RECORDS};
use super::*;
use crate::logging::{Home, FORMATTED_MARKER};
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::AtomicUsize;

const HOME: &str = "/home/canary-user-7f3a";

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

    /// The event of the `n`th envelope.
    fn event(&self, n: usize) -> Value {
        event_of(&self.0.lock().unwrap()[n])
    }
}

/// The event line of an envelope: its header, the item's header and the event, one JSON object per line.
fn event_of(envelope: &[u8]) -> Value {
    String::from_utf8_lossy(envelope)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value.get("exception").is_some())
        .expect("an event")
}

struct Refuses(AtomicUsize);

impl Post for Refuses {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(PostError)
    }
}

struct Breaks;

impl Post for Breaks {
    fn post(&self, _envelope: Vec<u8>, _budget: Duration) -> Result<(), PostError> {
        panic!("the transport broke");
    }
}

fn resource() -> Resource {
    Resource::of("sidevoice-desktop", "1.2.3", "linux", "x86_64")
}

fn config(dir: &Path, post: Option<Arc<dyn Post>>) -> Config {
    Config { resource: resource(), dsn: None, dir: dir.to_owned(), post }
}

fn start(dir: &Path, post: &Arc<Recorder>) -> Arc<Reporter> {
    tail::testing::own();
    let reporter = Reporter::start(config(dir, Some(post.clone())));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    reporter
}

fn home() -> Home {
    Home::new(Some(Path::new(HOME)))
}

fn image(name: &str) -> Image {
    Image {
        name: name.to_owned(),
        addr: 0x1000,
        size: 0x1000,
        vmaddr: 0,
        id: "0f0e0d0c-0b0a-0908-0706-050403020100".to_owned(),
        code_id: Some("0123456789abcdef".to_owned()),
    }
}

fn panic_with<'a>(literal: Option<&'a str>, formatted_len: usize, tail: Vec<String>) -> Panic<'a> {
    Panic {
        resource: resource(),
        at_ms: now_ms(),
        literal,
        formatted_len,
        file: "shared/core/src/voice.rs",
        line: 212,
        frames: vec![
            Frame {
                function: Some("sidevoice_shared::voice::open".into()),
                file: Some("voice.rs".into()),
                line: Some(7),
                addr: 0x1800,
            },
            Frame { function: None, file: None, line: None, addr: 0x1900 },
        ],
        images: vec![image("Sidevoice")],
        tail,
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn record(literal: Option<&str>) -> CrashRecord {
    tail::testing::own();
    CrashRecord::from_panic(
        panic_with(
            literal,
            if literal.is_some() { 0 } else { 42 },
            vec!["-1.250s WARN sidevoice_shared::voice: the call dropped device=d1".into()],
        ),
        &home(),
    )
}

fn records_in(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("crash-records"))
        .map(|entries| entries.flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".json")).count())
        .unwrap_or(0)
}

/// A reporter over `dir` that writes a record of its own now, as the capture thread does.
fn crashed(dir: &Path, literal: Option<&str>) -> Arc<Reporter> {
    tail::testing::own();
    let post: Arc<dyn Post> = Arc::new(Recorder::default());
    let store = Store::new(dir, home());
    let reporter = Reporter::with(resource(), Some(post), Some(store), Settings::load(dir), vec![image("Sidevoice")]);
    reporter.write(
        literal,
        if literal.is_some() { 0 } else { 42 },
        "shared/core/src/voice.rs",
        212,
        backtrace::Backtrace::new_unresolved(),
    );
    reporter
}

#[test]
fn a_record_is_sent_at_the_next_start_and_deleted_after_a_successful_send() {
    let dir = tempfile::tempdir().unwrap();
    crashed(dir.path(), Some("the microphone is gone"));
    assert_eq!(records_in(dir.path()), 1, "the panic left a record and sent nothing");

    let post = Arc::new(Recorder::default());
    start(dir.path(), &post);
    assert_eq!(post.count(), 1);
    assert_eq!(records_in(dir.path()), 0, "deleted after Sentry took it");
    let event = post.event(0);
    assert_eq!(event["exception"]["values"][0]["value"], "the microphone is gone");
    assert_eq!(event["release"], "sidevoice-desktop@1.2.3");
    assert_eq!(event["tags"]["service.name"], "sidevoice-desktop");

    // Sent once: the next start finds nothing.
    let again = Arc::new(Recorder::default());
    start(dir.path(), &again);
    assert_eq!(again.count(), 0);
}

#[test]
fn a_failed_send_keeps_the_record_for_the_next_start_and_stops_the_pass() {
    let dir = tempfile::tempdir().unwrap();
    crashed(dir.path(), Some("first"));
    crashed(dir.path(), Some("second"));
    assert_eq!(records_in(dir.path()), 2);
    let refuses = Arc::new(Refuses(AtomicUsize::new(0)));
    let reporter = Reporter::start(config(dir.path(), Some(refuses.clone())));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    assert_eq!(refuses.0.load(Ordering::SeqCst), 1, "the first failure ends the pass: the network is likely down");
    assert_eq!(records_in(dir.path()), 2);
    let post = Arc::new(Recorder::default());
    start(dir.path(), &post);
    assert_eq!(post.count(), 2);
    assert_eq!(records_in(dir.path()), 0);
}

#[test]
fn under_never_nothing_is_recorded_or_sent_and_what_was_pending_is_discarded() {
    let dir = tempfile::tempdir().unwrap();
    crashed(dir.path(), Some("left from before"));
    Settings::load(dir.path()).set_mode(Mode::Never).unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = start(dir.path(), &post);
    assert_eq!(reporter.mode(), Mode::Never);
    assert!(!reporter.is_open());
    assert_eq!(post.count(), 0);
    assert_eq!(records_in(dir.path()), 0, "records made while off, or left from before it, are discarded");
    // A panic now leaves nothing.
    reporter.write(Some("after never"), 0, "x.rs", 1, backtrace::Backtrace::new_unresolved());
    assert_eq!(records_in(dir.path()), 0);
}

#[test]
fn a_build_without_a_dsn_records_and_sends_nothing_but_keeps_the_setting() {
    let dir = tempfile::tempdir().unwrap();
    let reporter = Reporter::start(config(dir.path(), None));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    assert!(!reporter.is_open(), "nowhere to send to: nothing is recorded");
    assert_eq!(reporter.mode(), Mode::Auto);
    reporter.write(Some("dev build"), 0, "x.rs", 1, backtrace::Backtrace::new_unresolved());
    assert_eq!(records_in(dir.path()), 0);
    reporter.set_mode(Mode::Never).unwrap();
    assert_eq!(Settings::load(dir.path()).mode(), Mode::Never);
    reporter.set_mode(Mode::Auto).unwrap();
    assert!(!reporter.is_open(), "back to auto, still nowhere to send to");
}

#[test]
fn switching_to_never_closes_the_gate_erases_the_records_and_forgets_the_install_id_and_auto_makes_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = start(dir.path(), &post);
    let first = reporter.installation_id().unwrap();
    assert_eq!(reporter.installation_id().unwrap(), first, "what the page shows is what is sent");
    reporter.write(Some("one"), 0, "x.rs", 1, backtrace::Backtrace::new_unresolved());
    assert_eq!(records_in(dir.path()), 1);

    reporter.set_mode(Mode::Never).unwrap();
    assert!(!reporter.is_open());
    assert_eq!(reporter.installation_id(), None, "none is kept under never");
    assert_eq!(records_in(dir.path()), 0);
    let text = std::fs::read_to_string(dir.path().join(settings::FILE)).unwrap();
    assert!(!text.contains(&first), "{text}");
    reporter.write(Some("two"), 0, "x.rs", 1, backtrace::Backtrace::new_unresolved());
    assert_eq!(records_in(dir.path()), 0, "no record while off");

    reporter.set_mode(Mode::Auto).unwrap();
    assert!(reporter.is_open());
    let second = reporter.installation_id().unwrap();
    assert_ne!(second, first, "a new id when sending is turned back on");
    assert_eq!(second.len(), 32);
}

#[test]
fn the_id_of_a_report_is_the_one_the_page_would_show() {
    let dir = tempfile::tempdir().unwrap();
    crashed(dir.path(), Some("boom"));
    let post = Arc::new(Recorder::default());
    let reporter = start(dir.path(), &post);
    let shown = reporter.installation_id().unwrap();
    assert_eq!(post.event(0)["tags"]["installation.id"], shown.as_str());
}

#[test]
fn the_report_holds_the_closed_field_list_and_nothing_else() {
    let record = record(None);
    let bytes = report::envelope(report::event(&record, "0123456789abcdef0123456789abcdef")).unwrap();
    let event = event_of(&bytes);
    let allowed = ["event_id", "level", "timestamp", "platform", "release", "tags", "extra", "exception", "debug_meta"];
    let keys = |value: &Value| -> Vec<String> { value.as_object().unwrap().keys().cloned().collect() };
    for key in keys(&event) {
        assert!(allowed.contains(&key.as_str()), "unexpected field {key}: {event}");
    }
    for forbidden in [
        "server_name",
        "user",
        "contexts",
        "breadcrumbs",
        "modules",
        "request",
        "environment",
        "dist",
        "sdk",
        "message",
    ] {
        assert!(event.get(forbidden).is_none(), "{forbidden} must be unset: {event}");
    }
    for key in keys(&event["tags"]) {
        assert!(TAGS.contains(&key.as_str()), "{key}");
    }
    for key in keys(&event["extra"]) {
        assert!(EXTRAS.contains(&key.as_str()), "{key}");
    }
    assert_eq!(event["tags"]["os.type"], "linux");
    assert_eq!(event["tags"]["os.name"], "Linux");
    assert_eq!(event["tags"]["host.arch"], "amd64");
    assert_eq!(event["level"], "fatal");
    assert_eq!(event["extra"]["location"], "shared/core/src/voice.rs:212");
    assert_eq!(event["extra"]["len"], 42);
    let exception = &event["exception"]["values"][0];
    assert_eq!(exception["type"], "panic");
    assert_eq!(exception["value"], FORMATTED_MARKER);
    for key in keys(exception) {
        assert!(["type", "value", "stacktrace"].contains(&key.as_str()), "{key}");
    }
    for frame in exception["stacktrace"]["frames"].as_array().unwrap() {
        for key in keys(frame) {
            assert!(["function", "filename", "lineno", "instruction_addr"].contains(&key.as_str()), "{key}");
        }
        assert!(frame["instruction_addr"].is_string(), "every frame has its raw address: {frame}");
    }
    let image = &event["debug_meta"]["images"][0];
    assert_eq!(image["name"], "Sidevoice");
    for key in keys(image) {
        assert!(
            ["type", "name", "arch", "image_addr", "image_size", "image_vmaddr", "id", "code_id"]
                .contains(&key.as_str()),
            "{key}: {image}"
        );
    }
    assert_eq!(event["extra"]["log_tail"][0], "-1.250s WARN sidevoice_shared::voice: the call dropped device=d1");
}

#[test]
fn a_home_path_or_a_host_name_never_reaches_what_is_sent() {
    tail::testing::own();
    std::env::set_var("HOSTNAME", "canary-host-7f3a");
    std::env::set_var("COMPUTERNAME", "canary-host-7f3a");
    let literal = format!("{HOME}/Documents/secret.txt could not be read");
    let mut panic = panic_with(
        Some(&literal),
        0,
        vec![
            format!("-0.500s ERROR sidevoice_shared::engine: the model could not be loaded path={HOME}/models/x"),
            // Not a line of ours: a dependency's, with a path; dropped.
            format!("-0.400s ERROR hyper::client: connect error {HOME}/x"),
        ],
    );
    panic.file = "/home/canary-user-7f3a/.cargo/git/checkouts/x/src/lib.rs";
    panic.frames[0].file = Some(format!("{HOME}/src/lib.rs"));
    panic.images = vec![Image { name: format!("{HOME}/Applications/Sidevoice"), ..image("x") }];
    let record = CrashRecord::from_panic(panic, &home());
    let bytes = report::envelope(report::event(&record, "0123456789abcdef0123456789abcdef")).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for canary in ["canary-user", "canary-host", "/home/"] {
        assert!(!text.contains(canary), "{canary} in {text}");
    }
    let event = event_of(&bytes);
    assert_eq!(event["exception"]["values"][0]["value"], "~/Documents/secret.txt could not be read");
    assert_eq!(event["extra"]["location"], "unknown:212", "an absolute path is not sent");
    assert!(event.get("server_name").is_none());
    assert_eq!(event["extra"]["log_tail"].as_array().unwrap().len(), 1, "a line that is not ours is dropped");
    assert!(event["extra"]["log_tail"][0].as_str().unwrap().contains("path=~/models/x"));
    assert_eq!(event["debug_meta"]["images"][0]["name"], "Sidevoice", "a module's name is a bare file name");
    assert_eq!(event["exception"]["values"][0]["stacktrace"]["frames"][1]["filename"], "lib.rs");
}

#[test]
fn a_dependency_panic_keeps_its_place_in_the_registry_and_nothing_of_the_home() {
    let place = |file: &str| {
        let mut panic = panic_with(Some("x"), 0, Vec::new());
        panic.file = file;
        CrashRecord::from_panic(panic, &home()).location.file
    };
    assert_eq!(place("shared/core/src/crash/mod.rs"), "shared/core/src/crash/mod.rs");
    assert_eq!(
        place("/home/canary-user-7f3a/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.40.0/src/runtime/mod.rs"),
        "tokio-1.40.0/src/runtime/mod.rs"
    );
    assert_eq!(
        place(
            r"C:\Users\canary-user-7f3a\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tokio-1.40.0\src\lib.rs"
        ),
        "tokio-1.40.0/src/lib.rs"
    );
    for hostile in
        ["/home/canary-user-7f3a/x.rs", "../x.rs", "a/../x.rs", "C:\\Users\\canary-user-7f3a\\x.rs", "a b/x.rs", ""]
    {
        assert_eq!(place(hostile), "unknown", "{hostile}");
    }
}

#[test]
fn a_record_that_is_not_ours_does_not_parse_and_one_that_is_is_cleaned_again() {
    tail::testing::own();
    let good = serde_json::to_value(record(Some("ok"))).unwrap();
    let parse = |value: &Value| CrashRecord::parse(&serde_json::to_vec(value).unwrap(), &home());
    assert!(parse(&good).is_some());
    let with = |name: &str, value: Value| {
        let mut changed = good.clone();
        changed[name] = value;
        changed
    };
    assert!(parse(&with("extra", Value::from(1))).is_none(), "a field outside the closed list");
    assert!(parse(&with("id", Value::from("not-a-uuid"))).is_none());
    assert!(
        parse(&with("id", Value::from("0F0E0D0C-0B0A-4908-8706-050403020100"))).is_none(),
        "an id that is not canonical"
    );
    assert!(parse(&with("service", Value::from("a b"))).is_none());
    assert!(parse(&with("os_type", Value::from("x".repeat(40)))).is_none());
    assert!(CrashRecord::parse(b"not json", &home()).is_none());
    assert!(CrashRecord::parse(b"{}", &home()).is_none());
    // Cleaned again: a message that is not printable ASCII is the marker, a path is hidden, a bad line and a bad frame go.
    let mut changed = good.clone();
    changed["exception"]["message"] = Value::from("caf\u{e9} \u{202e}reversed");
    changed["location"]["file"] = Value::from(format!("{HOME}/x.rs"));
    changed["log_tail"] = Value::from(vec![
        "-1.000s WARN sidevoice_shared::x: fine",
        "2026-01-01 WARN sidevoice_shared::x: dated",
        "free text",
    ]);
    changed["backtrace"][0]["function"] = Value::from("evil;name");
    changed["images"][0]["name"] = Value::from("/etc/shadow");
    let cleaned = parse(&changed).unwrap();
    assert_eq!(cleaned.exception.message, FORMATTED_MARKER);
    assert_eq!(cleaned.location.file, "unknown");
    assert_eq!(cleaned.log_tail, ["-1.000s WARN sidevoice_shared::x: fine"]);
    assert_eq!(cleaned.backtrace[0].function, None);
    assert_eq!(cleaned.images[0].name, "shadow", "only the last component of a name is kept");
}

#[test]
fn a_formatted_message_is_never_kept_only_the_marker_and_its_length() {
    let formatted = record(None);
    assert_eq!(formatted.exception.message, FORMATTED_MARKER);
    assert_eq!(formatted.exception.len, Some(42));
    let literal = record(Some("a literal"));
    assert_eq!(literal.exception.message, "a literal");
    assert_eq!(literal.exception.len, None);
}

#[test]
fn the_store_is_bounded_in_count_and_age_and_keeps_the_newest() {
    tail::testing::own();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path(), home());
    let old = {
        let mut record = record(Some("old"));
        record.at_ms = now_ms() - 31 * 24 * 60 * 60 * 1000;
        record
    };
    store.record_without_waiting(&old, || true).unwrap();
    assert_eq!(records_in(dir.path()), 0, "past the age: gone at once");
    for n in 0..(MAX_RECORDS + 5) {
        let mut record = record(Some("fill"));
        record.at_ms = now_ms() - 1000 * (MAX_RECORDS as u64 + 5 - n as u64);
        store.record_without_waiting(&record, || true).unwrap();
    }
    assert_eq!(records_in(dir.path()), MAX_RECORDS);
    let pending = store.pending().unwrap();
    assert!(pending.windows(2).all(|pair| pair[0].at_ms >= pair[1].at_ms), "newest first");
    // `wanted` false: nothing is kept.
    store.record_without_waiting(&record(Some("no")), || false).unwrap();
    assert_eq!(records_in(dir.path()), MAX_RECORDS);
}

#[test]
fn garbage_in_the_records_directory_is_deleted_or_left_and_never_breaks_a_pass() {
    let dir = tempfile::tempdir().unwrap();
    crashed(dir.path(), Some("real"));
    let records = dir.path().join("crash-records");
    let id = record::random_uuid();
    std::fs::write(records.join(format!("crash-{id}.json")), b"not json").unwrap();
    std::fs::write(records.join("crash-0.json"), vec![b' '; 70_000]).unwrap();
    std::fs::write(records.join("crash-leftover.tmp"), b"half").unwrap();
    std::fs::write(records.join("notes.txt"), b"not a record's name").unwrap();
    // A record under another record's name is not that record.
    let other = serde_json::to_vec(&record(Some("copied"))).unwrap();
    std::fs::write(records.join(format!("crash-{}.json", record::random_uuid())), other).unwrap();
    let post = Arc::new(Recorder::default());
    start(dir.path(), &post);
    assert_eq!(post.count(), 1, "only the real one");
    assert!(records.join("notes.txt").exists(), "what is not named like a record is left alone");
    let names: Vec<String> = std::fs::read_dir(&records)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["notes.txt"], "{names:?}");
}

#[test]
fn a_transport_that_panics_is_contained_and_the_record_stays() {
    let dir = tempfile::tempdir().unwrap();
    crashed(dir.path(), Some("boom"));
    let reporter = Reporter::start(config(dir.path(), Some(Arc::new(Breaks))));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    assert_eq!(records_in(dir.path()), 1, "not sent, so not deleted");
    assert_eq!(reporter.mode(), Mode::Auto, "the reporter still answers");
    assert!(reporter.installation_id().is_some());
    // And the next start sends it.
    let post = Arc::new(Recorder::default());
    start(dir.path(), &post);
    assert_eq!(post.count(), 1);
}

#[test]
fn a_directory_that_is_not_one_leaves_reporting_off_and_the_app_working() {
    let file = tempfile::NamedTempFile::new().unwrap();
    // The app's directory is a regular file: nothing can be read or written there.
    let post = Arc::new(Recorder::default());
    tail::testing::own();
    let reporter = Reporter::start(config(file.path(), Some(post.clone())));
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    assert_eq!(reporter.mode(), Mode::Never, "a setting that cannot be read is never for that run");
    assert!(!reporter.is_open());
    assert!(reporter.set_mode(Mode::Auto).is_err(), "the choice was not kept, and the page is told so");
    assert_eq!(reporter.installation_id(), None);
    assert_eq!(post.count(), 0);

    let unavailable = Reporter::unavailable(resource());
    assert!(!unavailable.is_open());
    assert_eq!(unavailable.mode(), Mode::Never);
    assert!(unavailable.set_mode(Mode::Auto).is_err());
    assert!(unavailable.wait_idle(Duration::from_secs(1)));
}

#[test]
fn a_bad_dsn_is_reporting_off_for_the_run() {
    let dir = tempfile::tempdir().unwrap();
    for dsn in ["not a dsn", "", "https://"] {
        let reporter = Reporter::start(Config {
            resource: resource(),
            dsn: Some(dsn.to_owned()),
            dir: dir.path().to_owned(),
            post: None,
        });
        assert!(reporter.wait_idle(Duration::from_secs(10)));
        assert!(!reporter.is_open(), "{dsn:?}");
        assert_eq!(reporter.mode(), Mode::Auto);
    }
    let good = Config {
        resource: resource(),
        dsn: Some("https://public@sentry.invalid/1".to_owned()),
        dir: dir.path().to_owned(),
        post: None,
    };
    let reporter = Reporter::start(good);
    assert!(reporter.wait_idle(Duration::from_secs(10)));
    assert!(reporter.is_open(), "a build with a DSN records and sends");
}

#[test]
fn only_one_pass_runs_at_a_time_and_the_one_that_ends_frees_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = start(dir.path(), &post);
    reporter.sending.store(true, Ordering::SeqCst);
    reporter.begin_pass();
    assert!(!reporter.wait_idle(Duration::from_millis(50)), "the one under way is not replaced");
    reporter.sending.store(false, Ordering::SeqCst);
    reporter.begin_pass();
    assert!(reporter.wait_idle(Duration::from_secs(10)));
}

#[test]
fn the_switch_is_copied_to_the_core_the_app_installed_and_only_after_that_and_only_on_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = start(dir.path(), &post);
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    reporter.on_local_core_change(move |mode| sink.lock().unwrap().push(mode));
    // The app did not install a core (the person's own install, say): its switch is not copied anywhere.
    reporter.set_mode(Mode::Never).unwrap();
    reporter.set_mode(Mode::Auto).unwrap();
    assert!(heard.lock().unwrap().is_empty());
    // Once it did, the first copy takes the person's choice as it is now; later changes are told, repeats are not.
    reporter.set_mode(Mode::Never).unwrap();
    assert_eq!(reporter.local_core_installed(), Some(Mode::Never));
    reporter.set_mode(Mode::Never).unwrap();
    assert!(heard.lock().unwrap().is_empty());
    reporter.set_mode(Mode::Auto).unwrap();
    reporter.set_mode(Mode::Auto).unwrap();
    reporter.set_mode(Mode::Never).unwrap();
    assert_eq!(*heard.lock().unwrap(), [Mode::Auto, Mode::Never]);
    // A choice that was not kept is not copied.
    let failing = Reporter::unavailable(resource());
    let none = Arc::new(Mutex::new(0));
    let counter = none.clone();
    failing.on_local_core_change(move |_| *counter.lock().unwrap() += 1);
    assert!(failing.set_mode(Mode::Auto).is_err());
    assert_eq!(*none.lock().unwrap(), 0);
    // The marker is kept: a later run of the app still copies.
    let later = start(dir.path(), &post);
    let again = Arc::new(Mutex::new(Vec::new()));
    let sink = again.clone();
    later.on_local_core_change(move |mode| sink.lock().unwrap().push(mode));
    later.set_mode(Mode::Auto).unwrap();
    assert_eq!(*again.lock().unwrap(), [Mode::Auto]);
}

#[test]
fn a_real_stack_is_recorded_with_addresses_and_only_the_modules_that_hold_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    tail::testing::own();
    let post: Arc<dyn Post> = Arc::new(Recorder::default());
    let everything = Image { addr: 0, size: u64::MAX, ..image("holds-everything") };
    let nothing = Image { addr: 0, size: 1, ..image("holds-nothing") };
    let reporter = Reporter::with(
        resource(),
        Some(post),
        Some(Store::new(dir.path(), home())),
        Settings::load(dir.path()),
        vec![everything, nothing],
    );
    reporter.write(Some("real stack"), 0, "shared/core/src/crash/tests.rs", 1, backtrace::Backtrace::new_unresolved());
    let pending = Store::new(dir.path(), home()).pending().unwrap();
    let record = &pending[0];
    assert!(!record.backtrace.is_empty());
    assert!(record.backtrace.iter().all(|frame| frame.addr != 0), "the raw instruction address of each frame");
    assert_eq!(record.images.iter().map(|image| image.name.as_str()).collect::<Vec<_>>(), ["holds-everything"]);
}

#[test]
fn error_codes_for_the_log_exist() {
    for code in [
        SharedError::CrashReportNotSent,
        SharedError::CrashRecordsUnavailable,
        SharedError::CrashSettingsNotKept,
        SharedError::CrashSettingsUnreadable,
        SharedError::CrashReporterFailed,
    ] {
        assert!(code.code().starts_with("shared.crash-"), "{}", code.code());
    }
}
