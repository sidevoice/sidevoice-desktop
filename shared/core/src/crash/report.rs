//! A record as a Sentry event, and the POST.
//!
//! The event is built from the record's closed field list and nothing else ([`event`]); no SDK client, scope, integration or
//! transport exists in the process, only the SDK's protocol types, so nothing an SDK default would add (a host name, contexts,
//! breadcrumbs, modules, a user) can be in it:
//!
//! | Field | Content |
//! |---|---|
//! | `event_id` | The record's own id (a random UUID made when the panic happened) |
//! | `timestamp`, `level`, `platform` | When the panic happened, `fatal`, `native` |
//! | `release` | `<service.name>@<service.version>`: `sidevoice-desktop@x`, `sidevoice-mobile@x` |
//! | `tags` | `service.name`, `os.type`, `os.name`, `host.arch`, `installation.id` (the random id) |
//! | `exception` | `type` (`panic`), `value` (a literal or the marker), frames: `function`, a bare `filename`, `lineno`, `instruction_addr` |
//! | `debug_meta.images` | The loaded modules that hold a frame of the stack, for symbolication: bare name, address, size, debug id, code id |
//! | `extra` | `location` (`file:line` of the panic), `log_tail` (the last 20 warnings and errors of our targets, offsets from the crash, ids re-keyed), `len` (with the marker, how long the formatted message was) |
//!
//! `server_name`, `user`, `contexts`, `breadcrumbs`, `modules`, `request`, `environment`, `dist` and the SDK's own `sdk` are
//! unset. The event is the only item of its envelope.

use std::borrow::Cow;
use std::str::FromStr;
use std::time::{Duration, UNIX_EPOCH};

use sentry_types::protocol::v7::{
    Addr, DebugImage, DebugMeta, Envelope, Event, Exception, Frame, Level, Map, Stacktrace, SymbolicDebugImage, Value,
    Values,
};
use sentry_types::{CodeId, DebugId, Dsn, Uuid};

use super::record::CrashRecord;

/// The tags an event may carry.
pub const TAGS: [&str; 5] = ["service.name", "os.type", "os.name", "host.arch", "installation.id"];
/// The `extra` entries an event may carry.
pub const EXTRAS: [&str; 3] = ["location", "log_tail", "len"];

/// The event of a record, from its closed field list: the Sentry `event_id` is the record's id, the release the version that
/// crashed, and `installation.id` the app's random id.
pub fn event(record: &CrashRecord, installation_id: &str) -> Event<'static> {
    let tags: Map<String, String> = [
        ("service.name", record.service.as_str()),
        ("os.type", record.os_type.as_str()),
        ("os.name", record.os_name.as_str()),
        ("host.arch", record.arch.as_str()),
        ("installation.id", installation_id),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    let frames: Vec<Frame> = record
        .backtrace
        .iter()
        .map(|frame| Frame {
            function: frame.function.clone(),
            filename: frame.file.clone(),
            lineno: frame.line.map(u64::from),
            instruction_addr: Some(Addr(frame.addr)),
            ..Default::default()
        })
        .collect();
    let images: Vec<DebugImage> = record
        .images
        .iter()
        .filter_map(|image| {
            Some(DebugImage::Symbolic(SymbolicDebugImage {
                name: image.name.clone(),
                arch: None,
                image_addr: Addr(image.addr),
                image_size: image.size,
                image_vmaddr: Addr(image.vmaddr),
                id: DebugId::from_str(&image.id).ok()?,
                code_id: image.code_id.as_deref().map(|id| CodeId::new(id.to_owned())),
                debug_file: None,
            }))
        })
        .collect();
    let mut extra: Map<String, Value> = Map::new();
    extra.insert("location".to_owned(), Value::from(format!("{}:{}", record.location.file, record.location.line)));
    extra.insert("log_tail".to_owned(), Value::from(record.log_tail.clone()));
    if let Some(len) = record.exception.len {
        extra.insert("len".to_owned(), Value::from(len));
    }
    Event {
        event_id: Uuid::parse_str(&record.id).unwrap_or_else(|_| Uuid::nil()),
        level: Level::Fatal,
        platform: Cow::Borrowed("native"),
        // Never the host name: an SDK client would fill it in, and no client exists here; still said, not assumed.
        server_name: None,
        timestamp: UNIX_EPOCH + Duration::from_millis(record.at_ms),
        release: Some(Cow::Owned(format!("{}@{}", record.service, record.version))),
        tags,
        extra,
        exception: Values::from(vec![Exception {
            ty: record.exception.kind.clone(),
            value: Some(record.exception.message.clone()),
            stacktrace: Stacktrace::from_frames_reversed(frames),
            ..Default::default()
        }]),
        debug_meta: Cow::Owned(DebugMeta { sdk_info: None, images }),
        ..Default::default()
    }
}

/// The bytes of an envelope holding `event` alone.
pub fn envelope(event: Event<'static>) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    Envelope::from(event).to_writer(&mut bytes).ok()?;
    Some(bytes)
}

/// The DSN a build was given, if it parses.
pub fn parse_dsn(text: &str) -> Option<Dsn> {
    Dsn::from_str(text).ok()
}

/// The POST failed (no network, a refusal, a rate limit, a timeout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostError;

/// Where an envelope's bytes go; the real one posts to Sentry, a test keeps them. `budget` is the most it may take.
pub trait Post: Send + Sync + 'static {
    fn post(&self, envelope: Vec<u8>, budget: Duration) -> Result<(), PostError>;
}

/// The POST to the DSN's envelope endpoint.
pub struct SentryPost {
    url: String,
    auth: String,
}

impl SentryPost {
    pub fn new(dsn: &Dsn, user_agent: &str) -> SentryPost {
        SentryPost { url: dsn.envelope_api_url().to_string(), auth: dsn.to_auth(Some(user_agent)).to_string() }
    }
}

impl Post for SentryPost {
    /// On a thread of its own, one of the reporter's ([`super::reporter_thread`]): a panic in the HTTP or TLS code is never
    /// recorded as a crash of the app.
    fn post(&self, envelope: Vec<u8>, budget: Duration) -> Result<(), PostError> {
        let (url, auth) = (self.url.clone(), self.auth.clone());
        let (tell, hear) = std::sync::mpsc::channel();
        super::reporter_thread("crash-post", move || {
            let _ = tell.send(send(&url, &auth, envelope, budget));
        })
        .map_err(|_| PostError)?;
        match hear.recv_timeout(budget + Duration::from_millis(500)) {
            Ok(true) => Ok(()),
            _ => Err(PostError),
        }
    }
}

/// The request, on a runtime that has no thread but this one's and the pool it blocks DNS on, every one of them marked as the
/// reporter's (`reqwest`'s blocking client would run its own thread, which the panic hook could not tell from the app's).
fn send(url: &str, auth: &str, envelope: Vec<u8>, budget: Duration) -> bool {
    let Ok(runtime) =
        tokio::runtime::Builder::new_current_thread().enable_all().on_thread_start(super::mark_reporter_thread).build()
    else {
        return false;
    };
    runtime
        .block_on(async {
            let client = reqwest::Client::builder().timeout(budget).build().ok()?;
            let answer = client
                .post(url)
                .header("X-Sentry-Auth", auth)
                .header("Content-Type", "application/x-sentry-envelope")
                .body(envelope)
                .send()
                .await
                .ok()?;
            answer.status().is_success().then_some(())
        })
        .is_some()
}
