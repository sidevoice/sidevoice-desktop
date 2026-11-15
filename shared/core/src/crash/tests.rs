//! The rules of crash reports, tested at the points where they are kept: no send under `never`, nothing leaves after a
//! switch to it, the payload holds the closed fields only, a machine's record becomes an event of its own, and a record is
//! acknowledged only after a successful send.

use super::event::{self, EXTRAS, FORMATTED_MARKER, TAGS};
use super::reporter::{Config, Reporter, SendError};
use super::transport::{Gate, Post, PostError};
use super::{machine, Machine, MachineError, MachineRecord, Mode};
use crate::logging::Resource;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A test transport: keeps every envelope it is given, and answers as told.
#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<Vec<u8>>>,
    fail: std::sync::atomic::AtomicBool,
}

impl Post for Recorder {
    fn post(&self, envelope: Vec<u8>) -> Result<(), PostError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(PostError);
        }
        self.sent.lock().unwrap().push(envelope);
        Ok(())
    }
}

impl Recorder {
    fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }

    /// The event JSON of the `n`th envelope: header line, item header line, payload line.
    fn event(&self, n: usize) -> Value {
        let bytes = self.sent.lock().unwrap()[n].clone();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "one event and nothing else: {text}");
        assert_eq!(serde_json::from_str::<Value>(lines[1]).unwrap()["type"], "event");
        serde_json::from_str(lines[2]).unwrap()
    }
}

const RESOURCE: Resource = Resource {
    service_name: "sidevoice-desktop",
    service_version: "1.2.3",
    os_type: "darwin",
    os_name: "macOS",
    host_arch: "arm64",
};

fn reporter(dir: &std::path::Path, post: &Arc<Recorder>) -> Arc<Reporter> {
    Reporter::start(Config {
        resource: RESOURCE,
        dsn: None,
        dir: dir.to_owned(),
        post: Some(post.clone() as Arc<dyn Post>),
    })
}

fn record_json(id: &str) -> Value {
    json!({
        "id": id, "piece": "core", "version": "0.4.1", "os": "linux", "arch": "x86_64",
        "first_at": "2026-11-14T10:00:00.000Z", "last_at": "2026-11-14T10:05:00.000Z", "count": 3,
        "signature": "abc", "exception": {"type": "panic", "message": "a literal"},
        "location": {"file": "rust/x.rs", "line": 7},
        "backtrace": ["core::foo at rust/x.rs:7", "core::bar"],
        "log_tail": ["2026-11-14T10:04:59Z WARN something happened"],
        "unknown": "never copied"
    })
}

const ID: &str = "3f2b8c1e-5a47-4d1b-9c33-0a1b2c3d4e5f";

/// A machine that keeps what it was asked.
struct Fake {
    rows: Vec<Value>,
    listed: AtomicUsize,
    acknowledged: RefCell<Vec<String>>,
    refuse_ack: bool,
}

impl Fake {
    fn with(rows: Vec<Value>) -> Fake {
        Fake { rows, listed: AtomicUsize::new(0), acknowledged: RefCell::new(Vec::new()), refuse_ack: false }
    }
}

impl Machine for Fake {
    fn list(&self) -> Result<Vec<Value>, MachineError> {
        self.listed.fetch_add(1, Ordering::SeqCst);
        Ok(self.rows.clone())
    }

    fn acknowledge(&self, id: &str) -> Result<(), MachineError> {
        if self.refuse_ack {
            return Err(MachineError::Unavailable);
        }
        self.acknowledged.borrow_mut().push(id.to_owned());
        Ok(())
    }
}

#[test]
fn under_never_nothing_is_initialised_fetched_acknowledged_or_sent() {
    let dir = tempfile::tempdir().unwrap();
    super::Settings::load(dir.path()).set_mode(Mode::Never).unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = reporter(dir.path(), &post);
    assert_eq!(reporter.mode(), Mode::Never);
    assert!(!reporter.is_active(), "no client under never");
    reporter.capture_panic(Some("a literal"), "main");
    let record = MachineRecord::parse(&record_json(ID)).unwrap();
    assert_eq!(reporter.send_machine(&record), Err(SendError::Disabled));
    let machine = Fake::with(vec![record_json(ID)]);
    assert_eq!(machine::sync(&reporter, &machine), machine::Synced::default());
    assert_eq!(machine.listed.load(Ordering::SeqCst), 0, "a never app does not even ask the machine");
    assert!(machine.acknowledged.borrow().is_empty());
    assert_eq!(post.count(), 0);
}

#[test]
fn a_build_without_a_dsn_sends_nothing_but_keeps_the_setting() {
    let dir = tempfile::tempdir().unwrap();
    let reporter = Reporter::start(Config { resource: RESOURCE, dsn: None, dir: dir.path().to_owned(), post: None });
    assert_eq!(reporter.mode(), Mode::Auto);
    assert!(!reporter.is_active());
    reporter.capture_panic(Some("x"), "main");
    reporter.set_mode(Mode::Never).unwrap();
    assert_eq!(
        Reporter::start(Config { resource: RESOURCE, dsn: None, dir: dir.path().to_owned(), post: None }).mode(),
        Mode::Never
    );
}

#[test]
fn switching_to_never_stops_everything_at_once_and_forgets_the_install_id() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = reporter(dir.path(), &post);
    reporter.capture_panic(Some("first"), "main");
    assert_eq!(post.count(), 1);
    let id = post.event(0)["tags"]["installation.id"].as_str().unwrap().to_owned();
    // What is already handed to the transport when the person switches: the gate drops it.
    let gate_post = Arc::new(Recorder::default());
    let gate = Gate::new(gate_post.clone());
    gate.close();
    let envelope: sentry::Envelope = event::own_panic(
        &RESOURCE,
        "x",
        event::OwnPanic { literal: None, thread: "t", stacktrace: None, images: vec![] },
    )
    .into();
    sentry::Transport::send_envelope(&gate, envelope);
    assert_eq!(gate_post.count(), 0, "a closed gate sends nothing");
    reporter.set_mode(Mode::Never).unwrap();
    assert!(!reporter.is_active());
    reporter.capture_panic(Some("second"), "main");
    assert_eq!(post.count(), 1, "nothing after the switch");
    let file = std::fs::read_to_string(dir.path().join(super::settings::FILE)).unwrap();
    assert!(!file.contains(&id), "the install id is gone: {file}");
    // Back to auto: a fresh client and a new id.
    reporter.set_mode(Mode::Auto).unwrap();
    reporter.capture_panic(Some("third"), "main");
    assert_eq!(post.count(), 2);
    assert_ne!(post.event(1)["tags"]["installation.id"].as_str().unwrap(), id);
}

#[test]
fn an_own_panic_carries_the_closed_fields_only() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = reporter(dir.path(), &post);
    // Formatted payload: only the marker travels.
    reporter.capture_panic(None, "worker-1");
    let event = post.event(0);
    let allowed =
        ["event_id", "level", "platform", "timestamp", "release", "tags", "exception", "debug_meta", "sdk", "extra"];
    for key in event.as_object().unwrap().keys() {
        assert!(allowed.contains(&key.as_str()), "{key} is not on the closed list: {event}");
    }
    for key in event["tags"].as_object().unwrap().keys() {
        assert!(TAGS.contains(&key.as_str()), "{key}");
    }
    assert_eq!(event["release"], "sidevoice-desktop@1.2.3");
    assert_eq!(event["tags"]["service.name"], "sidevoice-desktop");
    assert_eq!(event["tags"]["thread.name"], "worker-1");
    assert_eq!(event["exception"]["values"][0]["value"], FORMATTED_MARKER);
    assert_eq!(event["exception"]["values"][0]["type"], "panic");
    for absent in
        ["user", "request", "contexts", "breadcrumbs", "server_name", "modules", "environment", "message", "logentry"]
    {
        assert!(event.get(absent).is_none(), "{absent}: {event}");
    }
    for frame in event["exception"]["values"][0]["stacktrace"]["frames"].as_array().into_iter().flatten() {
        for key in frame.as_object().unwrap().keys() {
            assert!(["function", "filename", "lineno", "instruction_addr"].contains(&key.as_str()), "{key}");
        }
    }
    // A literal travels as it is.
    reporter.capture_panic(Some("a literal"), "main");
    assert_eq!(post.event(1)["exception"]["values"][0]["value"], "a literal");
}

#[test]
fn sanitize_rebuilds_an_event_from_the_closed_list() {
    use sentry::protocol::{Breadcrumb, Context, Event, Exception, Frame, Request, Stacktrace, User};
    let mut dirty = Event {
        user: Some(User { id: Some("canary-user".into()), ip_address: Some(Default::default()), ..Default::default() }),
        request: Some(Request { url: Some("https://canary.example/?q=1".parse().unwrap()), ..Default::default() }),
        server_name: Some("canary-host".into()),
        environment: Some("canary-env".into()),
        message: Some("canary message".into()),
        ..Default::default()
    };
    dirty.contexts.insert("device".into(), Context::Other(Default::default()));
    dirty.breadcrumbs.values.push(Breadcrumb { message: Some("canary crumb".into()), ..Default::default() });
    dirty.tags.insert("canary".into(), "tag".into());
    dirty.tags.insert("os.type".into(), "linux".into());
    dirty.extra.insert("canary".into(), "x".into());
    dirty.modules.insert("canary".into(), "1".into());
    dirty.exception.values.push(Exception {
        ty: "panic".into(),
        value: Some("literal".into()),
        stacktrace: Some(Stacktrace {
            frames: vec![Frame {
                function: Some("f".into()),
                abs_path: Some("/home/canary-user/src/x.rs".into()),
                lineno: Some(3),
                vars: [("canary".to_owned(), Value::from("secret"))].into_iter().collect(),
                context_line: Some("canary line".into()),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    });
    let text = serde_json::to_string(&event::sanitize(dirty)).unwrap();
    assert!(!text.contains("canary-") && !text.contains("canary message") && !text.contains("canary crumb"), "{text}");
    assert!(!text.contains("/home/") && !text.contains("secret") && !text.contains("canary line"), "{text}");
    assert!(text.contains("\"filename\":\"x.rs\"") && text.contains("\"os.type\":\"linux\""), "{text}");
    assert!(!text.contains("\"canary\""), "{text}");
    let _ = EXTRAS;
}

#[test]
fn a_machine_record_is_its_own_event_with_the_records_id() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = reporter(dir.path(), &post);
    let record = MachineRecord::parse(&record_json(ID)).unwrap();
    reporter.send_machine(&record).unwrap();
    let event = post.event(0);
    assert_eq!(event["event_id"], ID.replace('-', ""), "the record's id is the event_id");
    assert_eq!(event["release"], "sidevoice-core@0.4.1", "its own release, not the app's");
    assert_eq!(event["tags"]["service.name"], "sidevoice-core");
    assert_eq!(event["tags"]["os.type"], "linux");
    assert_eq!(event["tags"]["host.arch"], "x86_64");
    assert!(event["tags"].get("installation.id").is_none(), "no app context in a machine's report");
    assert!(event["tags"].get("os.name").is_none());
    assert!(event["debug_meta"].get("images").is_none(), "no module of this app");
    assert_eq!(event["extra"]["count"], 3);
    assert_eq!(event["extra"]["log_tail"][0], "2026-11-14T10:04:59Z WARN something happened");
    let frames = event["exception"]["values"][0]["stacktrace"]["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["function"], "core::foo");
    assert_eq!(frames[1]["lineno"], 7);
    let text = event.to_string();
    assert!(!text.contains("never copied") && !text.contains("sidevoice-desktop") && !text.contains("1.2.3"), "{text}");
    let connector = MachineRecord::parse(&{
        let mut value = record_json(ID);
        value["piece"] = json!("connector");
        value
    })
    .unwrap();
    reporter.send_machine(&connector).unwrap();
    assert_eq!(post.event(1)["release"], "sidevoice-connector@0.4.1");
}

#[test]
fn a_record_that_is_not_ours_does_not_parse() {
    let mut bad_id = record_json(ID);
    bad_id["id"] = json!("not-a-uuid");
    let mut v1 = record_json(ID);
    v1["id"] = json!("3f2b8c1e-5a47-1d1b-9c33-0a1b2c3d4e5f");
    let mut piece = record_json(ID);
    piece["piece"] = json!("app");
    let mut control = record_json(ID);
    control["exception"]["message"] = json!("line\nbreak");
    let mut long = record_json(ID);
    long["backtrace"] = json!(vec!["f"; 65]);
    let mut time = record_json(ID);
    time["last_at"] = json!("yesterday");
    for bad in [bad_id, v1, piece, control, long, time, json!({}), json!("x")] {
        assert!(MachineRecord::parse(&bad).is_none(), "{bad}");
    }
}

#[test]
fn a_record_is_acknowledged_only_after_a_successful_send() {
    let dir = tempfile::tempdir().unwrap();
    let post = Arc::new(Recorder::default());
    let reporter = reporter(dir.path(), &post);
    let machine = Fake::with(vec![record_json(ID), json!({"junk": true})]);

    post.fail.store(true, Ordering::SeqCst);
    let failed = machine::sync(&reporter, &machine);
    assert_eq!((failed.sent, failed.failed, failed.skipped), (0, 1, 1));
    assert!(machine.acknowledged.borrow().is_empty(), "a failed send is not acknowledged");

    post.fail.store(false, Ordering::SeqCst);
    let sent = machine::sync(&reporter, &machine);
    assert_eq!((sent.sent, sent.failed), (1, 0));
    assert_eq!(*machine.acknowledged.borrow(), vec![ID.to_owned()]);
    assert_eq!(post.count(), 1);

    // The machine does not take the acknowledgement: counted, and sent again next time under the same id.
    let mut stubborn = Fake::with(vec![record_json(ID)]);
    stubborn.refuse_ack = true;
    let again = machine::sync(&reporter, &stubborn);
    assert_eq!((again.sent, again.unacknowledged), (1, 1));
    assert_eq!(post.event(0)["event_id"], post.event(1)["event_id"]);
}

#[test]
fn error_codes_for_the_log_exist() {
    assert!(crate::error::SharedError::CrashReportNotSent.code().starts_with("shared."));
}
