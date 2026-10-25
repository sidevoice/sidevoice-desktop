//! Remote pairing schema and refusal mapping. The shared platform crate owns every filesystem operation.

use serde_json::{json, Value};
use sidevoice_platform::fs::{Access, Directory, Files, Fs, HeldDirectory};
use std::io::{self, ErrorKind};
use std::path::Path;

type Answer = Result<Value, Value>;
const LIMIT: u64 = 64 * 1024;

fn refused(key: &str) -> Value {
    json!({"key": key})
}

fn storage_error(error: io::Error, fallback: &str) -> Value {
    refused(match error.kind() {
        ErrorKind::PermissionDenied | ErrorKind::InvalidInput => "reset.unsafe",
        ErrorKind::InvalidData => "data.pairings-invalid",
        _ => fallback,
    })
}

fn checked_pairings(value: &Value) -> bool {
    let Some(rows) = value.get("pairings").and_then(Value::as_array) else { return false };
    let pending = value.get("pendingRevokes").and_then(Value::as_array);
    if rows.len() > 32
        || pending.is_some_and(|rows| rows.len() > 32)
        || !serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= 64 * 1024)
    {
        return false;
    }
    let valid = |row: &Value| {
        row.get("local").is_none()
            && ["fp", "public_key", "device_id", "token"]
                .iter()
                .all(|field| row.get(*field).and_then(Value::as_str).is_some_and(|s| !s.is_empty() && s.len() <= 4096))
            && row.get("urls").and_then(Value::as_array).is_some_and(|urls| {
                urls.len() <= 16 && urls.iter().all(|url| url.as_str().is_some_and(|s| s.len() <= 2048))
            })
    };
    rows.iter().all(valid) && pending.is_none_or(|rows| rows.iter().all(valid))
}

pub(super) fn read_pairings(path: &Path) -> Answer {
    let parent = path.parent().ok_or_else(|| refused("reset.unsafe"))?;
    let name = path.file_name().ok_or_else(|| refused("reset.unsafe"))?;
    let bytes = match HeldDirectory::open(parent, Access::NotWritableByOthers) {
        // Discovery never creates an app directory.
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(storage_error(error, "data.pairings-read")),
        Ok(held) => held
            // Accept legacy inherited permissions; new writes are private from birth.
            .read(name, Access::NotWritableByOthers, LIMIT)
            .map_err(|error| storage_error(error, "data.pairings-read"))?,
    };
    match bytes {
        None => Ok(json!({"pairings": [], "pendingRevokes": []})),
        Some(bytes) => serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(checked_pairings)
            .ok_or_else(|| refused("data.pairings-invalid")),
    }
}

pub(super) fn write_pairings(path: &Path, value: &Value) -> Answer {
    write_pairings_with(path, value, || {})
}

fn write_pairings_with(path: &Path, value: &Value, before_publish: impl FnOnce()) -> Answer {
    if !checked_pairings(value) {
        return Err(refused("data.pairings-invalid"));
    }
    let parent = path.parent().ok_or_else(|| refused("reset.unsafe"))?;
    let name = path.file_name().ok_or_else(|| refused("reset.unsafe"))?;
    let held = match HeldDirectory::open(parent, Access::NotWritableByOthers) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            Files::ensure_private_dir(parent).map_err(|error| storage_error(error, "data.pairings-write"))?;
            HeldDirectory::open(parent, Access::NotWritableByOthers)
        }
        result => result,
    }
    .map_err(|error| storage_error(error, "data.pairings-write"))?;
    let bytes = serde_json::to_vec(value).map_err(|_| refused("data.pairings-invalid"))?;
    before_publish();
    held.write(name, &bytes).map_err(|error| storage_error(error, "data.pairings-write"))?;
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn pairings_refuse_symlinked_parent_and_file() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join("app")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("app/redirect")).unwrap();
        let redirected = home.path().join("app/redirect/pairings.json");
        assert_eq!(read_pairings(&redirected), Err(refused("reset.unsafe")));
        assert_eq!(write_pairings(&redirected, &json!({"pairings": []})), Err(refused("reset.unsafe")));
        let file = home.path().join("app/pairings.json");
        std::os::unix::fs::symlink(outside.path().join("other.json"), &file).unwrap();
        assert_eq!(read_pairings(&file), Err(refused("reset.unsafe")));
        assert!(!outside.path().join("other.json").exists());
    }

    #[test]
    fn pending_revocations_share_the_private_native_pairings_file() {
        let home = tempfile::tempdir().unwrap();
        let file = fs::canonicalize(home.path()).unwrap().join("app/pairings.json");
        let pending = json!({"fp":"fp", "public_key":"pk", "device_id":"id", "token":"live-secret", "urls":["https://example.test"]});
        let value = json!({"pairings":[], "pendingRevokes":[pending]});
        assert_eq!(write_pairings(&file, &value), Ok(Value::Null));
        assert_eq!(read_pairings(&file), Ok(value));
        Files::check_private(&Files::open_read(&file).unwrap()).unwrap();
        Files::check_private(&Files::open_read(file.parent().unwrap()).unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pairing_write_refuses_replaced_ancestor_before_rename() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let outside = fs::canonicalize(outside.path()).unwrap();
        let app = home.join("app");
        fs::create_dir(&app).unwrap();
        fs::write(outside.join("sentinel"), b"unrelated").unwrap();
        let file = app.join("pairings.json");
        let value = json!({"pairings": [], "pendingRevokes": []});
        let result = write_pairings_with(&file, &value, || {
            fs::rename(&app, home.join("moved-app")).unwrap();
            std::os::unix::fs::symlink(&outside, &app).unwrap();
        });
        assert_eq!(result, Err(refused("reset.unsafe")));
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"unrelated");
        assert!(!outside.join("pairings.json").exists());
    }

    #[test]
    fn absent_pairings_read_does_not_create_directories() {
        let home = tempfile::tempdir().unwrap();
        let app = fs::canonicalize(home.path()).unwrap().join("missing/app");
        assert_eq!(read_pairings(&app.join("pairings.json")), Ok(json!({"pairings": [], "pendingRevokes": []})));
        assert!(!app.parent().unwrap().exists());
    }

    #[test]
    fn malformed_or_oversized_records_are_invalid() {
        let home = tempfile::tempdir().unwrap();
        let app = fs::canonicalize(home.path()).unwrap().join("app");
        Files::ensure_private_dir(&app).unwrap();
        let held = HeldDirectory::open(&app, Access::Private).unwrap();
        let file = app.join("pairings.json");
        for bytes in [b"not json".to_vec(), vec![b' '; LIMIT as usize + 1], b"{}".to_vec()] {
            held.write("pairings.json".as_ref(), &bytes).unwrap();
            assert_eq!(read_pairings(&file), Err(refused("data.pairings-invalid")));
        }
    }

    #[test]
    fn invalid_pairing_never_replaces_saved_credentials() {
        let home = tempfile::tempdir().unwrap();
        let file = fs::canonicalize(home.path()).unwrap().join("app/pairings.json");
        let saved = json!({"pairings": [], "pendingRevokes": []});
        write_pairings(&file, &saved).unwrap();
        for invalid in [json!({}), json!({"pairings": [{}]}), json!({"pairings": [], "pendingRevokes": [{}]})] {
            assert_eq!(write_pairings(&file, &invalid), Err(refused("data.pairings-invalid")));
            assert_eq!(read_pairings(&file), Ok(saved.clone()));
        }
        // Exercise replacement of an existing record (including Windows atomic replacement).
        let next = json!({"pairings": [], "pendingRevokes": [], "future": true});
        write_pairings(&file, &next).unwrap();
        assert_eq!(read_pairings(&file), Ok(next));
    }

    #[test]
    fn hard_linked_pairings_are_refused_without_changing_the_other_name() {
        let home = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(home.path()).unwrap();
        let file = root.join("app/pairings.json");
        let value = json!({"pairings": []});
        write_pairings(&file, &value).unwrap();
        let alias = root.join("other.json");
        fs::hard_link(&file, &alias).unwrap();
        let original = fs::read(&alias).unwrap();
        assert_eq!(read_pairings(&file), Err(refused("reset.unsafe")));
        assert_eq!(write_pairings(&file, &value), Err(refused("reset.unsafe")));
        assert_eq!(fs::read(alias).unwrap(), original);
    }

    // Permission bits and symlink replacement are Unix-specific attack mechanisms.
    #[cfg(unix)]
    #[test]
    fn existing_readable_app_directory_is_not_chmodded() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let app = fs::canonicalize(home.path()).unwrap().join("app");
        fs::create_dir(&app).unwrap();
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
        let file = app.join("pairings.json");
        let value = json!({"pairings": []});
        fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_pairings(&file), Ok(value.clone()));
        write_pairings(&file, &value).unwrap();
        assert_eq!(fs::metadata(&app).unwrap().permissions().mode() & 0o777, 0o755);
        Files::check_private(&Files::open_read(&file).unwrap()).unwrap();
        fs::set_permissions(&app, fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(read_pairings(&file), Err(refused("reset.unsafe")));
        assert_eq!(write_pairings(&file, &value), Err(refused("reset.unsafe")));
    }

    #[cfg(unix)]
    #[test]
    fn pairing_write_refuses_replaced_grandparent_with_a_real_directory() {
        let home = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(home.path()).unwrap();
        let ancestor = root.join("ancestor");
        let file = ancestor.join("app/pairings.json");
        let value = json!({"pairings": []});
        write_pairings(&file, &value).unwrap();
        let original = fs::read(&file).unwrap();
        let moved = root.join("moved");
        let result = write_pairings_with(&file, &value, || {
            fs::rename(&ancestor, &moved).unwrap();
            fs::create_dir_all(ancestor.join("app")).unwrap();
            fs::write(&file, b"decoy").unwrap();
        });
        assert_eq!(result, Err(refused("reset.unsafe")));
        assert_eq!(fs::read(&file).unwrap(), b"decoy");
        assert_eq!(fs::read(moved.join("app/pairings.json")).unwrap(), original);
    }

    // Windows must deny ancestor renames for the entire shared write, even above the immediate parent.
    #[cfg(windows)]
    #[test]
    fn pairing_write_holds_every_windows_ancestor_until_publication() {
        let home = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(home.path()).unwrap();
        let ancestor = root.join("ancestor");
        let app = ancestor.join("app");
        let file = app.join("pairings.json");
        let value = json!({"pairings": []});
        let moved = root.join("moved");
        write_pairings_with(&file, &value, || {
            assert!(fs::rename(&app, &moved).is_err());
            assert!(fs::rename(&ancestor, &moved).is_err());
        })
        .unwrap();
        assert_eq!(read_pairings(&file), Ok(value));
        fs::rename(&ancestor, &moved).unwrap();
    }
}
