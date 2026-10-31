//! The pairings the room page keeps (the machines it is paired with and the revocations still to tell them), as one JSON
//! file in the app's private storage: read and written whole, as the page's JSON, with the refusals the page already
//! knows from the desktop (`data.pairings-invalid`, `-read`, `-write`; docs/BRIDGE.md). The shape of the same
//! `read`/`write` pair as the shared plugin's `PairingStore`, which this app implements when the plugin joins it
//! (docs/PAIRING_STORAGE.md → "On the phone").
//!
//! A write is atomic: the new record is written to a temporary file in the same directory, synced, renamed over the old
//! one and the directory synced (`atomic-write-file`), so a crash or a power loss leaves the old record or the new one,
//! never half of either.
//!
//! Nothing here defends against another process of the same user, as the desktop's held directory does: on a phone the
//! file is in the app's own sandbox (the data directory only this app can open), and no other app can reach it.
use atomic_write_file::AtomicWriteFile;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{ErrorKind, Read, Write},
    path::PathBuf,
};

/// The most a record may hold, read or written (the desktop's bound).
const LIMIT: u64 = 64 * 1024;

type Answer = Result<Value, Value>;

fn refused(key: &str) -> Value {
    json!({ "key": key })
}

/// The pairings file of the app, and what makes a record pairings.
pub struct PairingFile {
    path: PathBuf,
    /// Whether a record has the shape of pairings. The shape is the room page's contract, not this file's: the app hands
    /// in the one check it shares with the desktop.
    valid: fn(&Value) -> bool,
}

impl PairingFile {
    /// The record at `path`; its directory is created by the first write.
    pub fn new(path: PathBuf, valid: fn(&Value) -> bool) -> Self {
        Self { path, valid }
    }

    /// The pairings as the page wrote them last, or the empty set when none were. A record that is too big, not JSON or
    /// not pairings is refused `data.pairings-invalid`, and is left as it is.
    pub fn read(&self) -> Answer {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(json!({ "pairings": [], "pendingRevokes": [] }))
            }
            Err(_) => return Err(refused("data.pairings-read")),
        };
        let mut bytes = Vec::new();
        // One byte past the bound tells a record that is too big from one that is exactly the bound.
        file.take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| refused("data.pairings-read"))?;
        if bytes.len() as u64 > LIMIT {
            return Err(refused("data.pairings-invalid"));
        }
        serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(|value| (self.valid)(value))
            .ok_or_else(|| refused("data.pairings-invalid"))
    }

    /// Keeps `value` after checking its shape and its size; refuses what is not pairings (`data.pairings-invalid`) and
    /// what could not be kept (`data.pairings-write`), changing nothing.
    pub fn write(&self, value: &Value) -> Answer {
        let bytes = serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?;
        if !(self.valid)(value) || bytes.len() as u64 > LIMIT {
            return Err(refused("data.pairings-invalid"));
        }
        let failed = |_| refused("data.pairings-write");
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory).map_err(failed)?;
        }
        let mut file = AtomicWriteFile::open(&self.path).map_err(failed)?;
        file.write_all(&bytes).map_err(failed)?;
        file.commit().map_err(failed)?;
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the shared check: rows under `pairings`, each with a `token`.
    fn pairings(value: &Value) -> bool {
        value
            .get("pairings")
            .and_then(Value::as_array)
            .is_some_and(|rows| {
                rows.iter()
                    .all(|row| row.get("token").and_then(Value::as_str).is_some())
            })
    }

    fn file_in(directory: &tempfile::TempDir) -> PairingFile {
        PairingFile::new(directory.path().join("app/pairings.json"), pairings)
    }

    #[test]
    fn nothing_kept_is_the_empty_set_and_a_read_creates_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        assert_eq!(
            store.read(),
            Ok(json!({ "pairings": [], "pendingRevokes": [] }))
        );
        assert!(!home.path().join("app").exists());
    }

    #[test]
    fn what_is_written_is_read_back_and_replaced_whole() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let first = json!({ "pairings": [{ "token": "a" }], "pendingRevokes": [], "future": true });
        assert_eq!(store.write(&first), Ok(Value::Null));
        assert_eq!(store.read(), Ok(first));
        let next = json!({ "pairings": [] });
        assert_eq!(store.write(&next), Ok(Value::Null));
        assert_eq!(store.read(), Ok(next));
    }

    #[test]
    fn a_write_leaves_the_record_alone_in_its_directory() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        store.write(&json!({ "pairings": [] })).unwrap();
        store
            .write(&json!({ "pairings": [{ "token": "t" }] }))
            .unwrap();
        let names: Vec<_> = fs::read_dir(home.path().join("app"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["pairings.json"], "no temporary file is left behind");
    }

    #[test]
    fn a_refused_write_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let saved = json!({ "pairings": [{ "token": "kept" }] });
        store.write(&saved).unwrap();
        let too_big = json!({ "pairings": [{ "token": "x".repeat(LIMIT as usize) }] });
        for invalid in [json!({}), json!({ "pairings": [{}] }), json!([]), too_big] {
            assert_eq!(store.write(&invalid), Err(refused("data.pairings-invalid")));
            assert_eq!(store.read(), Ok(saved.clone()));
        }
    }

    #[test]
    fn a_record_that_is_not_pairings_is_refused_and_left_as_it_is() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        fs::create_dir(home.path().join("app")).unwrap();
        let path = home.path().join("app/pairings.json");
        for bytes in [
            b"not json".to_vec(),
            b"{}".to_vec(),
            vec![b' '; LIMIT as usize + 1],
        ] {
            fs::write(&path, &bytes).unwrap();
            assert_eq!(store.read(), Err(refused("data.pairings-invalid")));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn a_record_of_exactly_the_bound_is_kept() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let around = serde_json::to_vec(&json!({ "pairings": [{ "token": "" }] }))
            .unwrap()
            .len();
        let record = json!({ "pairings": [{ "token": "x".repeat(LIMIT as usize - around) }] });
        assert_eq!(serde_json::to_vec(&record).unwrap().len() as u64, LIMIT);
        assert_eq!(store.write(&record), Ok(Value::Null));
        assert_eq!(store.read(), Ok(record));
    }

    #[test]
    fn what_cannot_be_opened_or_kept_is_a_failure_not_an_invalid_record() {
        let home = tempfile::tempdir().unwrap();
        // The record's path is a directory: not a file to read, not one to replace.
        fs::create_dir_all(home.path().join("app/pairings.json")).unwrap();
        let store = file_in(&home);
        assert_eq!(store.read(), Err(refused("data.pairings-read")));
        assert_eq!(
            store.write(&json!({ "pairings": [] })),
            Err(refused("data.pairings-write"))
        );
    }

    #[test]
    fn a_file_where_the_directory_should_be_fails_the_write() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("app"), b"in the way").unwrap();
        assert_eq!(
            file_in(&home).write(&json!({ "pairings": [] })),
            Err(refused("data.pairings-write"))
        );
    }
}
