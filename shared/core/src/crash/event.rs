//! The closed field list, and the two events built from it.
//!
//! An event carries exactly these fields and no others ([`sanitize`] rebuilds every event from them just before it goes,
//! whatever the SDK or a scope added):
//!
//! | Field | Content |
//! |---|---|
//! | `event_id` | The machine record's own id (dedup across apps), or a random one for the app's own panic |
//! | `timestamp`, `level`, `platform` | When, `fatal`, `native` |
//! | `release` | `<service.name>@<version>`: `sidevoice-desktop@x`, `sidevoice-core@x`, `sidevoice-connector@x` |
//! | `tags` | `service.name`, `os.type`, `os.name`, `host.arch`, `thread.name`, `installation.id` (the app's own events only) |
//! | `exception` | `type`, `value` (a literal or the marker), frames: `function`, `filename` (a bare name), `lineno`, `instruction_addr` |
//! | `debug_meta.images` | Loaded modules for symbolication: bare name, `arch`, address, size, debug id, code id (the app's own events only) |
//! | `extra` | `count`, `first_at`, `log_tail` (a machine's record only: its own masked warnings and errors) |
//! | `sdk` | The SDK's name and version |
//!
//! A machine's record goes as its own event: its own release and `service.name`, nothing of this app (no installation id,
//! no app version, no OS of this device).

use super::machine::MachineRecord;
use crate::logging::Resource;
use sentry::protocol::{
    DebugImage, DebugMeta, Event, Exception, Frame, Level, Map, Stacktrace, SymbolicDebugImage, Value, Values,
};
use sentry::types::Uuid;
use std::borrow::Cow;
use std::time::SystemTime;

/// What a panic's message is replaced by when the payload is not a literal (`docs/logging.md`, section 9).
pub const FORMATTED_MARKER: &str = "<formatted panic message omitted>";

/// The tags an event may carry.
pub const TAGS: &[&str] = &["service.name", "os.type", "os.name", "host.arch", "thread.name", "installation.id"];
/// The `extra` entries an event may carry.
pub const EXTRAS: &[&str] = &["count", "first_at", "log_tail"];

/// The app's own panic, as the hook knows it.
pub struct OwnPanic<'a> {
    /// The payload when it is a `&'static str`; `None` for anything formatted or not text.
    pub literal: Option<&'a str>,
    /// The thread's name, as a log field takes it.
    pub thread: &'a str,
    /// The frames, with addresses, from `sentry`'s backtrace integration.
    pub stacktrace: Option<Stacktrace>,
    /// The loaded modules.
    pub images: Vec<DebugImage>,
}

fn tag(tags: &mut Map<String, String>, name: &str, value: &str) {
    tags.insert(name.to_owned(), value.to_owned());
}

fn event(
    release: String,
    timestamp: SystemTime,
    id: Uuid,
    tags: Map<String, String>,
    exception: Exception,
) -> Event<'static> {
    Event {
        event_id: id,
        level: Level::Fatal,
        platform: Cow::Borrowed("native"),
        timestamp,
        release: Some(Cow::Owned(release)),
        tags,
        exception: Values::from(vec![exception]),
        ..Default::default()
    }
}

/// The event of the app's own panic.
pub fn own_panic(resource: &Resource, installation_id: &str, panic: OwnPanic<'_>) -> Event<'static> {
    let mut tags = Map::new();
    tag(&mut tags, "service.name", resource.service_name);
    tag(&mut tags, "os.type", resource.os_type);
    tag(&mut tags, "os.name", resource.os_name);
    tag(&mut tags, "host.arch", resource.host_arch);
    tag(&mut tags, "thread.name", crate::logging::client_id(panic.thread));
    tag(&mut tags, "installation.id", installation_id);
    let exception = Exception {
        ty: "panic".to_owned(),
        value: Some(panic.literal.unwrap_or(FORMATTED_MARKER).to_owned()),
        stacktrace: panic.stacktrace,
        ..Default::default()
    };
    let mut event = event(
        format!("{}@{}", resource.service_name, resource.service_version),
        SystemTime::now(),
        random_uuid(),
        tags,
        exception,
    );
    event.debug_meta = Cow::Owned(DebugMeta { sdk_info: None, images: panic.images });
    sanitize(event)
}

/// A machine's record as its own event: `event_id` is the record's id.
pub fn machine_record(record: &MachineRecord) -> Event<'static> {
    let service = record.service_name();
    let mut tags = Map::new();
    tag(&mut tags, "service.name", service);
    tag(&mut tags, "os.type", &record.os);
    tag(&mut tags, "host.arch", &record.arch);
    let frames = record
        .backtrace
        .iter()
        .map(|line| {
            // `symbol` or `symbol at path:line`.
            match line.split_once(" at ") {
                Some((symbol, place)) => {
                    let (file, number) = place.rsplit_once(':').unwrap_or((place, ""));
                    Frame {
                        function: Some(symbol.to_owned()),
                        filename: Some(file.to_owned()),
                        lineno: number.parse().ok(),
                        ..Default::default()
                    }
                }
                None => Frame { function: Some(line.clone()), ..Default::default() },
            }
        })
        .collect::<Vec<_>>();
    let exception = Exception {
        ty: record.exception_type.clone(),
        value: Some(record.message.clone()),
        stacktrace: Stacktrace::from_frames_reversed(frames),
        ..Default::default()
    };
    let mut event = event(format!("{service}@{}", record.version), record.last_at, record.id, tags, exception);
    event.extra.insert("count".to_owned(), Value::from(record.count));
    event.extra.insert("first_at".to_owned(), Value::from(rfc3339(record.first_at)));
    event.extra.insert("log_tail".to_owned(), Value::from(record.log_tail.clone()));
    sanitize(event)
}

fn rfc3339(at: SystemTime) -> String {
    time::OffsetDateTime::from(at).format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

/// A random UUID v4 from the OS.
pub fn random_uuid() -> Uuid {
    let mut bytes = [0u8; 16];
    // With no randomness the id is still unique enough for a crash report: the clock's nanoseconds.
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        bytes = nanos.to_le_bytes();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// A bare file name: the last component of a path, so that no directory (a home, an install place) is sent.
fn bare(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let clean =
        !name.is_empty() && name.len() <= 100 && name.chars().all(|c| c.is_ascii_alphanumeric() || "_.-+ ".contains(c));
    clean.then(|| name.to_owned())
}

fn frame(frame: &Frame) -> Frame {
    Frame {
        function: frame.function.clone().map(|text| text.chars().take(240).collect()),
        filename: frame.filename.as_deref().or(frame.abs_path.as_deref()).and_then(bare),
        lineno: frame.lineno,
        instruction_addr: frame.instruction_addr,
        ..Default::default()
    }
}

fn image(image: &DebugImage) -> Option<DebugImage> {
    // `debug_images()` makes symbolic images; anything else is not ours to send.
    let DebugImage::Symbolic(image) = image else { return None };
    Some(DebugImage::Symbolic(SymbolicDebugImage {
        name: bare(&image.name)?,
        arch: image.arch.clone(),
        image_addr: image.image_addr,
        image_size: image.image_size,
        image_vmaddr: image.image_vmaddr,
        id: image.id,
        code_id: image.code_id.clone(),
        debug_file: image.debug_file.as_deref().and_then(bare),
    }))
}

/// The event rebuilt from the closed field list: whatever else it held is gone. This is the last step before an event
/// leaves (`before_send`), and the events above pass through it as well.
pub fn sanitize(event: Event<'static>) -> Event<'static> {
    let exceptions = event
        .exception
        .values
        .iter()
        .map(|exception| Exception {
            ty: exception.ty.chars().take(64).collect(),
            value: exception.value.clone().map(|text| text.chars().take(200).collect()),
            stacktrace: exception.stacktrace.as_ref().map(|trace| Stacktrace {
                frames: trace.frames.iter().take(128).map(frame).collect(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let tags = event.tags.into_iter().filter(|(name, _)| TAGS.contains(&name.as_str())).collect();
    let extra = event.extra.into_iter().filter(|(name, _)| EXTRAS.contains(&name.as_str())).collect();
    let images: Vec<DebugImage> = event.debug_meta.images.iter().filter_map(image).collect();
    Event {
        event_id: event.event_id,
        level: event.level,
        platform: Cow::Borrowed("native"),
        timestamp: event.timestamp,
        release: event.release,
        tags,
        extra,
        exception: Values::from(exceptions),
        debug_meta: Cow::Owned(DebugMeta { sdk_info: None, images }),
        sdk: event.sdk,
        ..Default::default()
    }
}
