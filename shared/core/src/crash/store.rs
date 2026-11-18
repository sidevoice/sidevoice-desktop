//! The records a panic leaves: one small file each, in `crash-records/` of the app's own directory, bounded, and tolerant of
//! anything it finds there that is not a record.
//!
//! A record is made at panic time and read at the next start; it is deleted once it was sent, or when the person chooses not to
//! send (`erase`). At most [`MAX_RECORDS`] are kept, taking at most [`MAX_BYTES`], none older than [`MAX_AGE_DAYS`]: the oldest
//! go first. One lock orders a capture, a read and an erase; the capture only tries it (a thread that panicked
//! inside the store holds it, and the panic hook may not wait).

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};

use super::record::CrashRecord;
use crate::logging::Home;

/// The records' directory, inside the app's own.
const DIR: &str = "crash-records";
/// The most records kept.
pub const MAX_RECORDS: usize = 20;
/// The most bytes the records take together.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// The longest a record is kept, from its crash.
pub const MAX_AGE_DAYS: u64 = 30;
/// The most bytes one record file may have: a bigger one is not ours and is deleted.
pub const MAX_RECORD_BYTES: u64 = 64 * 1024;

pub struct Store {
    dir: PathBuf,
    /// The one lock of the store: the app has one store, so one lock orders a capture, a read and an erase.
    lock: Mutex<()>,
    /// The home directory, hidden again in what is read back.
    home: Home,
}

/// A record on disk and the bytes it takes.
struct Kept {
    record: CrashRecord,
    bytes: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

impl Store {
    /// The store in the app's directory `app`.
    pub fn new(app: &Path, home: Home) -> Store {
        Store { dir: app.join(DIR), lock: Mutex::new(()), home }
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("crash-{id}.json"))
    }

    /// Keeps `record` unless `wanted` says no. `wanted` is asked under the lock, so that a record that raced the erasing of a
    /// "No enviar" (which takes the same lock) is not left behind. A lock somebody else holds means the record is dropped, never
    /// waited for: this is called from the panic's capture, where there is nobody to tell.
    pub fn record_without_waiting(&self, record: &CrashRecord, wanted: impl FnOnce() -> bool) -> io::Result<()> {
        let _held = match self.lock.try_lock() {
            Ok(held) => held,
            Err(TryLockError::Poisoned(held)) => held.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(io::ErrorKind::WouldBlock.into()),
        };
        if !wanted() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(record).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too large"));
        }
        fs::create_dir_all(&self.dir)?;
        // Written beside the file and renamed: a crash of the capture leaves no half record under a record's name.
        let temporary = self.dir.join(format!("crash-{}.tmp", record.id));
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, self.file(&record.id))?;
        let mut kept = self.load()?;
        self.enforce(&mut kept);
        Ok(())
    }

    /// The records waiting to be sent, newest first; what is expired or unreadable is deleted.
    pub fn pending(&self) -> io::Result<Vec<CrashRecord>> {
        let _held = self.lock();
        let mut kept = self.load()?;
        self.enforce(&mut kept);
        kept.sort_by_key(|item| std::cmp::Reverse(item.record.at_ms));
        Ok(kept.into_iter().map(|kept| kept.record).collect())
    }

    /// Deletes the record `id`, sent or not wanted. Only a canonical UUID names a file: nothing else reaches the file system.
    pub fn remove(&self, id: &str) -> io::Result<()> {
        if sentry_types::Uuid::parse_str(id).map(|uuid| uuid.to_string()).as_deref() != Ok(id) {
            return Ok(());
        }
        let _held = self.lock();
        match fs::remove_file(self.file(id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Deletes every record. A directory that is written into while it is removed (a capture that had the lock before this one)
    /// is removed again; one that still is after a few tries is an error (`DirectoryNotEmpty`), which the caller reports: the
    /// switch is already off, nothing new is kept, and the next start erases again.
    pub fn erase(&self) -> io::Result<()> {
        let _held = self.lock();
        let mut last: io::Error = io::ErrorKind::DirectoryNotEmpty.into();
        for _ in 0..5 {
            match fs::remove_dir_all(&self.dir) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => last = error,
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }

    /// Every valid record on disk. What is named like a record but is not one (too big, not JSON, not the closed shape, not named
    /// by its own id) is deleted, and so is a leftover temporary file; what is not named like either is left alone.
    fn load(&self) -> io::Result<Vec<Kept>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut kept = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with("crash-") && name.ends_with(".tmp") {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            let Some(id) = name.strip_prefix("crash-").and_then(|rest| rest.strip_suffix(".json")) else { continue };
            match read(&entry.path(), &self.home).filter(|(record, _)| record.id == id) {
                Some((record, bytes)) => kept.push(Kept { record, bytes }),
                None => {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Ok(kept)
    }

    /// Deletes what is past the age, then the oldest until the count and the size fit.
    fn enforce(&self, kept: &mut Vec<Kept>) {
        let limit = now_ms().saturating_sub(MAX_AGE_DAYS * 24 * 60 * 60 * 1000);
        let remove = |kept: &Kept| {
            let _ = fs::remove_file(self.file(&kept.record.id));
        };
        kept.retain(|item| {
            let old = item.record.at_ms < limit;
            if old {
                remove(item);
            }
            !old
        });
        kept.sort_by_key(|item| item.record.at_ms);
        while kept.len() > MAX_RECORDS || kept.iter().map(|item| item.bytes).sum::<u64>() > MAX_BYTES {
            let oldest = kept.remove(0);
            remove(&oldest);
        }
    }
}

/// The record in `path` and its size, `None` when it is not one: not a regular file, over the size, not JSON, or not the closed
/// shape.
fn read(path: &Path, home: &Home) -> Option<(CrashRecord, u64)> {
    // A link is not followed, and nothing but a regular file is read.
    if !fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path).ok()?.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    CrashRecord::parse(&bytes, home).map(|record| (record, bytes.len() as u64))
}
