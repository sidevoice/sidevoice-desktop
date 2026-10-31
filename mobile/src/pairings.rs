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
//! The pairings the room page keeps (the machines it is paired with and the revocations still to tell them), as one JSON
//! file in the app's private storage: the mobile app's `PairingStore` (sidevoice-app-tauri), read and written whole, as the
//! page's JSON, with the refusals the page already knows from the desktop (`data.pairings-invalid`, `-read`, `-write`;
//! docs/BRIDGE.md). The app manages one for the plugin: `Pairings(Arc::new(PairingFile::of_app(app)?))`.
//!
//! A write is atomic: the new record is written to a temporary file in the same directory, synced, renamed over the old
//! one and the directory synced (`atomic-write-file`), so a crash or a power loss leaves the old record or the new one,
//! never half of either.
//!
//! Nothing here defends against another process of the same user, as the desktop's held directory does: on a phone the
//! file is in the app's own sandbox (the data directory only this app can open), and no other app can reach it.
use crate::platform::app_files;
use atomic_write_file::AtomicWriteFile;
use serde_json::{json, Value};
use sidevoice_app_tauri::PairingStore;
use std::{
    fs::{self, File},
    io::{ErrorKind, Read, Write},
    path::PathBuf,
};
use tauri::{AppHandle, Manager, Runtime};

/// The most a record may hold, read or written (the desktop's bound).
const LIMIT: u64 = 64 * 1024;

/// The record's file name, in the app's own files.
const FILE: &str = "pairings.json";

type Answer = Result<Value, Value>;

fn refused(key: &str) -> Value {
    json!({ "key": key })
}

/// Whether `value` has the shape of pairings. The same check as the desktop's (`checked_pairings` in
/// `desktop/src/data/pairings.rs`): the room page's contract is one, on either host. The desktop's file is also built
/// without Tauri by `test-support`, which cannot reach a shared copy in `app-tauri` (it would link a webview), so the two
/// stay copies of each other until the shape has a home both can reach.
fn checked_pairings(value: &Value) -> bool {
    let Some(rows) = value.get("pairings").and_then(Value::as_array) else {
        return false;
    };
    let pending = value.get("pendingRevokes").and_then(Value::as_array);
    if rows.len() > 32
        || pending.is_some_and(|rows| rows.len() > 32)
        || !serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() as u64 <= LIMIT)
    {
        return false;
    }
    let valid = |row: &Value| {
        row.get("local").is_none()
            && ["fp", "public_key", "device_id", "token"]
                .iter()
                .all(|field| {
                    row.get(*field)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty() && s.len() <= 4096)
                })
            && row
                .get("urls")
                .and_then(Value::as_array)
                .is_some_and(|urls| {
                    urls.len() <= 16
                        && urls
                            .iter()
                            .all(|url| url.as_str().is_some_and(|s| s.len() <= 2048))
                })
    };
    rows.iter().all(valid) && pending.is_none_or(|rows| rows.iter().all(valid))
}

/// The pairings file of the app.
pub struct PairingFile {
    path: PathBuf,
}

impl PairingFile {
    /// The app's record, in its own private files: Application Support's app folder on iOS, `filesDir` on Android
    /// (`platform::app_files`), from Tauri's path resolver. Nothing is created until the first write.
    pub fn of_app<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Self> {
        let mut path = app.path().app_data_dir()?;
        if let Some(files) = app_files::FILES {
            path.push(files);
        }
        Ok(Self::new(path.join(FILE)))
    }

    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl PairingStore for PairingFile {
    /// The pairings as the page wrote them last, or the empty set when none were. A record that is too big, not JSON or
    /// not pairings is refused `data.pairings-invalid`, and is left as it is.
    fn read(&self) -> Answer {
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
            .filter(checked_pairings)
            .ok_or_else(|| refused("data.pairings-invalid"))
    }

    /// Keeps `value` after checking its shape and its size; refuses what is not pairings (`data.pairings-invalid`) and
    /// what could not be kept (`data.pairings-write`), changing nothing.
    fn write(&self, value: &Value) -> Answer {
        let bytes = serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?;
        if !checked_pairings(value) || bytes.len() as u64 > LIMIT {
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

    fn row(token: &str) -> Value {
        json!({ "fp": "fp", "public_key": "pk", "device_id": "id", "token": token, "urls": ["https://example.test"] })
    }

    fn file_in(directory: &tempfile::TempDir) -> PairingFile {
        PairingFile::new(directory.path().join("app").join(FILE))
    }

    #[test]
    fn the_shape_is_the_desktops() {
        let mut local = row("t");
        local["local"] = json!(true);
        let mut empty = row("");
        empty["urls"] = json!(vec!["https://example.test"; 17]);
        assert!(checked_pairings(&json!({ "pairings": [] })));
        assert!(checked_pairings(
            &json!({ "pairings": [row("t")], "pendingRevokes": [row("u")], "future": true })
        ));
        for not in [
            json!({}),
            json!([]),
            json!({ "pairings": [{}] }),
            json!({ "pairings": [], "pendingRevokes": [{}] }),
            json!({ "pairings": [local] }),
            json!({ "pairings": [empty] }),
            json!({ "pairings": vec![row("t"); 33] }),
        ] {
            assert!(!checked_pairings(&not), "{not}");
        }
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
        let first = json!({ "pairings": [row("a")], "pendingRevokes": [], "future": true });
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
        store.write(&json!({ "pairings": [row("t")] })).unwrap();
        let names: Vec<_> = fs::read_dir(home.path().join("app"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE], "no temporary file is left behind");
    }

    #[test]
    fn a_refused_write_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let store = file_in(&home);
        let saved = json!({ "pairings": [row("kept")] });
        store.write(&saved).unwrap();
        let too_big = json!({ "pairings": [], "pad": "x".repeat(LIMIT as usize) });
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
        let path = home.path().join("app").join(FILE);
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
        let around = serde_json::to_vec(&json!({ "pairings": [], "pad": "" }))
            .unwrap()
            .len();
        let record = json!({ "pairings": [], "pad": "x".repeat(LIMIT as usize - around) });
        assert_eq!(serde_json::to_vec(&record).unwrap().len() as u64, LIMIT);
        assert_eq!(store.write(&record), Ok(Value::Null));
        assert_eq!(store.read(), Ok(record));
    }

    #[test]
    fn what_cannot_be_opened_or_kept_is_a_failure_not_an_invalid_record() {
        let home = tempfile::tempdir().unwrap();
        // The record's path is a directory: not a file to read, not one to replace.
        fs::create_dir_all(home.path().join("app").join(FILE)).unwrap();
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
