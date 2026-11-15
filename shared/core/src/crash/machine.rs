//! The crash records of the machines the app is connected to: fetched, sent as events of their own, and acknowledged
//! only after the send succeeded (sidevoice-core `GET /api/device/crashes`, `DELETE /api/device/crashes/{id}`, with the
//! paired device's bearer token: `docs` of the core's `crash` module).
//!
//! - An app set to `never` neither fetches nor acknowledges: [`sync`] does not call the machine at all.
//! - A record is read again here ([`MachineRecord::parse`]): only the closed list is taken, every text bounded and free of
//!   control characters, the id a UUID v4, the message printable ASCII or the marker, and every tail line a warning or an
//!   error of the piece's own log with its time as an offset from the crash ([`super::tail`]). One that does not parse is skipped, never sent and never acknowledged.
//! - Send first, acknowledge after. A failed send leaves the record for the next pass; a failed acknowledgement means the
//!   next pass sends it again under the same `event_id`, which Sentry keeps once.

use super::reporter::Reporter;
use crate::error::SharedError;
use sentry::types::Uuid;
use serde_json::Value;
use std::time::SystemTime;

/// Which piece of a machine wrote the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Core,
    Connector,
}

/// A machine's record as the app holds it: the closed field list, validated.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineRecord {
    pub id: Uuid,
    pub piece: Piece,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub first_at: SystemTime,
    pub last_at: SystemTime,
    pub count: u32,
    pub exception_type: String,
    pub message: String,
    pub backtrace: Vec<String>,
    pub log_tail: Vec<String>,
}

const MAX_FRAMES: usize = 64;
const MAX_TAIL: usize = 20;

fn token(value: &str, max: usize, allowed: impl Fn(char) -> bool) -> bool {
    !value.is_empty() && value.len() <= max && value.chars().all(allowed)
}

fn text(value: &Value, max: usize) -> Option<String> {
    let text = value.as_str()?;
    (text.chars().count() <= max && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

fn lines(value: &Value, most: usize, max: usize) -> Option<Vec<String>> {
    let rows = value.as_array()?;
    (rows.len() <= most).then_some(())?;
    rows.iter().map(|row| text(row, max)).collect()
}

fn time_of(value: &Value) -> Option<SystemTime> {
    let at = time::OffsetDateTime::parse(value.as_str()?, &time::format_description::well_known::Rfc3339).ok()?;
    Some(at.into())
}

impl MachineRecord {
    /// The record `value` is, or `None` when it is not one of ours.
    pub fn parse(value: &Value) -> Option<MachineRecord> {
        let id = Uuid::parse_str(value["id"].as_str()?).ok()?;
        if id.get_version_num() != 4 || id.hyphenated().to_string() != value["id"].as_str()? {
            return None;
        }
        let piece = match value["piece"].as_str()? {
            "core" => Piece::Core,
            "connector" => Piece::Connector,
            _ => return None,
        };
        let version = text(&value["version"], 64)?;
        let os = text(&value["os"], 32)?;
        let arch = text(&value["arch"], 32)?;
        let exception_type = text(&value["exception"]["type"], 64)?;
        let is_version = |c: char| c.is_ascii_alphanumeric() || "._+-".contains(c);
        let is_name = |c: char| c.is_ascii_alphanumeric() || "_-".contains(c);
        if !token(&version, 64, is_version)
            || !token(&os, 32, is_name)
            || !token(&arch, 32, is_name)
            || !token(&exception_type, 64, |c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return None;
        }
        let message = text(&value["exception"]["message"], 200)?;
        if message != super::event::FORMATTED_MARKER && !message.bytes().all(|b| (0x20..0x7f).contains(&b)) {
            return None;
        }
        let log_tail = lines(&value["log_tail"], MAX_TAIL, 600)?;
        if !log_tail.iter().all(|line| super::tail::is_tail_line(line, piece)) {
            return None;
        }
        let (first_at, last_at) = (time_of(&value["first_at"])?, time_of(&value["last_at"])?);
        Some(MachineRecord {
            id,
            piece,
            version,
            os,
            arch,
            first_at: first_at.min(last_at),
            last_at: first_at.max(last_at),
            count: u32::try_from(value["count"].as_u64().unwrap_or(1).clamp(1, u64::from(u32::MAX))).unwrap_or(1),
            exception_type,
            message,
            backtrace: lines(&value["backtrace"], MAX_FRAMES, 240)?,
            log_tail,
        })
    }

    /// The `service.name` of the piece (also its release's name).
    pub fn service_name(&self) -> &'static str {
        match self.piece {
            Piece::Core => "sidevoice-core",
            Piece::Connector => "sidevoice-connector",
        }
    }
}

/// Why a machine could not be read or told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineError {
    /// Not connected, not paired, or it did not answer.
    Unavailable,
}

/// A machine, as the app reaches it: its records, and the acknowledgement of one.
pub trait Machine {
    /// The machine's records, as it listed them (JSON objects, not yet trusted).
    fn list(&self) -> Result<Vec<Value>, MachineError>;
    /// Tells the machine the record `id` (canonical hyphenated UUID) was handled, which deletes it. A record already gone
    /// is not an error.
    fn acknowledge(&self, id: &str) -> Result<(), MachineError>;
}

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    pub sent: usize,
    /// Records that could not be sent now (the transport failed): left where they are.
    pub failed: usize,
    /// Sent, but the machine did not take the acknowledgement: sent again next time under the same id.
    pub unacknowledged: usize,
    /// Records that are not ours to send (they did not parse).
    pub skipped: usize,
}

/// One pass over `machine`'s records. Under `never` it does nothing and does not call the machine.
pub fn sync(reporter: &Reporter, machine: &dyn Machine) -> Synced {
    let mut done = Synced::default();
    if !reporter.is_active() {
        return done;
    }
    let Ok(rows) = machine.list() else {
        tracing::debug!("a machine's crash records were not listed: it did not answer");
        return done;
    };
    for row in &rows {
        // The setting may change while a pass runs: look again before each record.
        if !reporter.is_active() {
            break;
        }
        let Some(record) = MachineRecord::parse(row) else {
            done.skipped += 1;
            continue;
        };
        match reporter.send_machine(&record) {
            Ok(()) => {
                done.sent += 1;
                tracing::info!(
                    piece = record.service_name(),
                    count = record.count,
                    "a machine's crash record was sent"
                );
                if machine.acknowledge(&record.id.hyphenated().to_string()).is_err() {
                    done.unacknowledged += 1;
                    tracing::warn!(
                        error.type = SharedError::CrashRecordNotAcknowledged.code(),
                        "a machine's crash record was sent but the machine did not take the acknowledgement; it will be sent again"
                    );
                }
            }
            Err(_) => {
                done.failed += 1;
                tracing::warn!(
                    error.type = SharedError::CrashReportNotSent.code(),
                    piece = record.service_name(),
                    "a machine's crash record could not be sent; it stays on the machine for the next try"
                );
            }
        }
    }
    done
}
