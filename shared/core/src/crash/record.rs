//! The record a panic leaves, with the closed field list it is made of, and the checks every record passes before it is kept or
//! sent (docs/CRASH_REPORTS.md; the logging guideline, section 9).
//!
//! A record is written at panic time and read at the next start. It is a file in the app's own directory, and a file may have
//! been changed since: reading it takes only the closed list, bounds and cleans every text, and keeps only what has the shape
//! of what we write. The panic message is a `&'static str` literal or the fixed marker with the formatted message's length,
//! never the formatted text; paths are bare file names or short relative paths; the log tail is lines of our own targets.

use std::str::FromStr;

use sentry_types::DebugId;
use serde::{Deserialize, Serialize};

use super::tail;
use super::{MAX_FRAMES, MAX_IMAGES, TAIL_LINES};
use crate::logging::{Home, Resource, FORMATTED_MARKER};

const MAX_TEXT: usize = 200;
const MAX_FUNCTION: usize = 240;
const MAX_NAME: usize = 100;
const MAX_LINE: usize = 600;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    /// `panic`.
    #[serde(rename = "type")]
    pub kind: String,
    /// A `&'static str` payload, or [`FORMATTED_MARKER`].
    pub message: String,
    /// With the marker: how long the formatted message was (its text is never kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub file: String,
    pub line: u32,
}

/// One frame of the stack, innermost first. The address is what Sentry symbolicates with the debug files of the release; the
/// name and place are what the binary itself could say (its symbol table, `strip = "debuginfo"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// A bare file name, never a path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub addr: u64,
}

/// A loaded module that holds a frame of the stack, so that Sentry can match the frame to the debug file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    /// A bare file name, never a path.
    pub name: String,
    pub addr: u64,
    pub size: u64,
    pub vmaddr: u64,
    /// The debug id (`debugid`'s canonical text).
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_id: Option<String>,
}

/// One crash. Exactly these fields are stored and sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrashRecord {
    /// A random UUID v4, made when the crash happened: the Sentry `event_id`, so a send repeated after a failed delete is one
    /// event.
    pub id: String,
    /// `service.name`, `service.version`, `os.type`, `os.name`, `host.arch`: what the app was.
    pub service: String,
    pub version: String,
    pub os_type: String,
    pub os_name: String,
    pub arch: String,
    /// Unix milliseconds, UTC.
    pub at_ms: u64,
    pub exception: Exception,
    pub location: Location,
    pub backtrace: Vec<Frame>,
    pub images: Vec<Image>,
    /// The last warnings and errors of our own targets (`tail`): each line starts with its time as an offset from the crash
    /// (`-12.345s`, not a date) and names the ids it carries by tokens of this record (`d1`, `c2`), so that nothing in it
    /// correlates with another report or says when the app was in use.
    pub log_tail: Vec<String>,
}

/// What a panic is, as the capture knows it.
pub struct Panic<'a> {
    pub resource: Resource,
    pub at_ms: u64,
    /// The payload when it is a `&'static str`; `None` for anything formatted.
    pub literal: Option<&'a str>,
    /// How long a formatted payload was; 0 when there was none.
    pub formatted_len: usize,
    pub file: &'a str,
    pub line: u32,
    pub frames: Vec<Frame>,
    pub images: Vec<Image>,
    /// Already rendered, with offsets from the crash (`tail::snapshot`).
    pub tail: Vec<String>,
}

impl CrashRecord {
    /// The record of `panic`, with the home directory shown as `~` in every text that could hold it.
    pub fn from_panic(panic: Panic<'_>, home: &Home) -> Self {
        let hide = |text: &str| home.redact(text);
        let mut record = Self {
            id: random_uuid(),
            service: panic.resource.service_name.to_owned(),
            version: panic.resource.service_version.to_owned(),
            os_type: panic.resource.os_type.to_owned(),
            os_name: panic.resource.os_name.to_owned(),
            arch: panic.resource.host_arch.to_owned(),
            at_ms: panic.at_ms,
            exception: Exception {
                kind: "panic".to_owned(),
                message: hide(panic.literal.unwrap_or(FORMATTED_MARKER)),
                len: u32::try_from(panic.formatted_len).ok().filter(|len| *len > 0 && panic.literal.is_none()),
            },
            location: Location { file: hide(panic.file), line: panic.line },
            backtrace: panic.frames,
            images: panic.images,
            log_tail: panic.tail.iter().map(|line| hide(line)).collect(),
        };
        record.normalize();
        record
    }

    /// A record read from a file: `None` unless it is the closed shape with a well-formed id and tokens. Every text is bounded
    /// and cleaned again, and the home directory is hidden again.
    pub fn parse(bytes: &[u8], home: &Home) -> Option<Self> {
        let mut record: CrashRecord = serde_json::from_slice(bytes).ok()?;
        record.exception.message = home.redact(&record.exception.message);
        record.location.file = home.redact(&record.location.file);
        for line in &mut record.log_tail {
            *line = home.redact(line);
        }
        let valid = canonical_uuid(&record.id)
            && token(&record.service, 64, |c| c.is_ascii_alphanumeric() || "._-".contains(c))
            && token(&record.version, 64, |c| c.is_ascii_alphanumeric() || "._+-".contains(c))
            && token(&record.os_type, 32, |c| c.is_ascii_alphanumeric() || "_-".contains(c))
            && token(&record.os_name, 32, |c| c.is_ascii_alphanumeric() || "_-".contains(c))
            && token(&record.arch, 32, |c| c.is_ascii_alphanumeric() || "_-".contains(c))
            && record.exception.kind == "panic";
        if !valid {
            return None;
        }
        record.normalize();
        Some(record)
    }

    /// Bounds every text and keeps only what has the shape of what we write: the message is printable ASCII or the marker, the
    /// file a short relative path, a frame a symbol and a bare file name, an image a bare name and a debug id, a log line one of
    /// our own warnings or errors.
    fn normalize(&mut self) {
        let message = clean(&self.exception.message, MAX_TEXT);
        self.exception.message =
            if message.bytes().all(|b| (0x20..0x7f).contains(&b)) { message } else { FORMATTED_MARKER.to_owned() };
        if self.exception.message != FORMATTED_MARKER {
            self.exception.len = None;
        }
        self.location.file = relative(&self.location.file);
        self.backtrace = self.backtrace.iter().take(MAX_FRAMES).map(frame).collect();
        self.images = self.images.iter().filter_map(image).take(MAX_IMAGES).collect();
        let lines: Vec<String> =
            self.log_tail.iter().map(|line| clean(line, MAX_LINE)).filter(|line| tail::is_tail_line(line)).collect();
        let skip = lines.len().saturating_sub(TAIL_LINES);
        self.log_tail = lines[skip..].to_vec();
    }
}

fn canonical_uuid(id: &str) -> bool {
    sentry_types::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
}

/// A random UUID v4 from the OS.
pub(super) fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    // With no randomness the id is still unique enough for a crash report: the clock's nanoseconds.
    if getrandom::fill(&mut bytes).is_err() {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        bytes = nanos.to_le_bytes();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    sentry_types::Uuid::from_bytes(bytes).to_string()
}

fn token(value: &str, max: usize, allowed: impl Fn(char) -> bool) -> bool {
    !value.is_empty() && value.len() <= max && value.chars().all(allowed)
}

/// The last component of a path, when it has the shape of a file name: so that no directory (a home, an install place) is ever
/// kept.
pub(super) fn bare(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    token(name, MAX_NAME, |c| c.is_ascii_alphanumeric() || "_.-+".contains(c)).then(|| name.to_owned())
}

/// A source path as the compiler gave it, kept only as a short relative path: ours (`shared/core/src/x.rs`) as it is, a
/// dependency's from the registry as `<crate>-<version>/src/x.rs` (what follows `registry/src/<index>/`); anything else, an
/// absolute path or one that is not a plain path, is `unknown`.
fn relative(path: &str) -> String {
    let path = clean(path, MAX_TEXT).replace('\\', "/");
    let parts: Vec<&str> = path.split('/').collect();
    let tail = parts
        .windows(3)
        .position(|window| window[0] == "registry" && window[1] == "src")
        .map(|at| parts[at + 3..].join("/"))
        .unwrap_or_else(|| path.clone());
    let plain = !tail.starts_with('/')
        && tail.split('/').all(|part| part != ".." && !part.is_empty())
        && tail.chars().all(|c| c.is_ascii_alphanumeric() || "_./-+".contains(c));
    if plain {
        tail
    } else {
        "unknown".to_owned()
    }
}

/// A frame with its name bounded and shaped like a symbol, its file a bare name: what is not is dropped, the address stays.
fn frame(frame: &Frame) -> Frame {
    Frame {
        function: frame.function.as_deref().map(|function| clean(function, MAX_FUNCTION)).filter(|function| {
            !function.is_empty()
                && function.chars().all(|c| c.is_ascii_alphanumeric() || "_:<>{}$.,&*()[]'+-@# ".contains(c))
        }),
        file: frame.file.as_deref().and_then(bare),
        line: frame.line,
        addr: frame.addr,
    }
}

/// An image with a bare name and a debug id `debugid` reads; `None` when it has neither.
fn image(image: &Image) -> Option<Image> {
    Some(Image {
        name: bare(&image.name)?,
        addr: image.addr,
        size: image.size,
        vmaddr: image.vmaddr,
        id: DebugId::from_str(&image.id).ok()?.to_string(),
        code_id: image.code_id.clone().filter(|id| token(id, 128, |c| c.is_ascii_hexdigit())),
    })
}

/// `text` with control characters as spaces and invisible format characters (bidi controls among them) removed, cut to at most
/// `max` characters.
pub(super) fn clean(text: &str, max: usize) -> String {
    text.chars().filter(|c| !is_format(*c)).map(|c| if c.is_control() { ' ' } else { c }).take(max).collect()
}

/// Unicode format characters (category Cf): they change how a text is shown without being seen.
fn is_format(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{110bd}'
        | '\u{1d173}'..='\u{1d17a}' | '\u{e0000}'..='\u{e007f}')
}
