//! Where the core and the connector listen (SEAMS §1, §2, §4): read from the records they publish, never worked out.
//! The core writes `endpoint` in `C/core.json`; the connector writes `{pid, endpoint}` in `D/connector.json` once it
//! listens and removes it when it exits cleanly. `endpoint` is the record `sidevoice-platform`'s `ipc` writes (an
//! absolute socket path on Unix, in the user's runtime directory), parsed back with its own type.
//!
//! A record that is not there, or a connector record whose `pid` is dead (it stays after a crash), is no host:
//! [`Check::Missing`]. One that is there but cannot be trusted or understood is a refusal.

use crate::checks::{self, Check};
use crate::paths::DataDirs;
use crate::Refusal;
use serde_json::Value;
use sidevoice_platform::ipc::Endpoint;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const RECORD_LIMIT: u64 = 64 * 1024;

/// The core's socket, from `C/core.json`. The caller has checked that `C` is private; the record lives in it.
pub fn core(dirs: &DataDirs) -> Result<PathBuf, Check> {
    let record = read(&dirs.core_record())?;
    socket(&record, &dirs.core_record())
}

/// The connector's socket, from `D/connector.json`, when its `pid` is alive. The caller has checked `D` and the record
/// with [`checks::owned_not_writable`].
pub fn connector(dirs: &DataDirs) -> Result<PathBuf, Check> {
    let path = dirs.connector_record();
    checks::owned_not_writable(&dirs.data)?;
    checks::owned_not_writable(&path)?;
    let record = read(&path)?;
    let pid = record.get("pid").and_then(Value::as_u64).and_then(|pid| i32::try_from(pid).ok()).filter(|pid| *pid > 0);
    match pid {
        Some(pid) if alive(pid) => socket(&record, &path),
        Some(_) => Err(Check::Missing),
        None => Err(invalid(&path, "no pid")),
    }
}

fn read(path: &Path) -> Result<Value, Check> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Check::Missing),
        Err(e) => return Err(invalid(path, &e.to_string())),
    };
    let mut bytes = Vec::new();
    file.take(RECORD_LIMIT + 1).read_to_end(&mut bytes).map_err(|e| invalid(path, &e.to_string()))?;
    if bytes.len() as u64 > RECORD_LIMIT {
        return Err(invalid(path, "too large"));
    }
    serde_json::from_slice(&bytes).map_err(|e| invalid(path, &e.to_string()))
}

fn socket(record: &Value, path: &Path) -> Result<PathBuf, Check> {
    let text = record.get("endpoint").and_then(Value::as_str).ok_or_else(|| invalid(path, "no endpoint"))?;
    let endpoint: Endpoint = text.parse().map_err(|e: io::Error| invalid(path, &e.to_string()))?;
    Ok(PathBuf::from(endpoint.to_string()))
}

fn invalid(path: &Path, why: &str) -> Check {
    Check::Unsafe(Refusal::new(
        "endpoint.record-invalid",
        format!("{} does not say where it listens ({why}); the app does not connect to it.", path.display()),
    ))
}

/// Whether process `pid` exists (signal 0: nothing is sent). Not permitted still means it is there.
fn alive(pid: i32) -> bool {
    // SAFETY: kill with signal 0 only checks; it has no other effect.
    unsafe { libc::kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dirs() -> (tempfile::TempDir, DataDirs) {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("d");
        std::fs::create_dir_all(data.join("core")).unwrap();
        (tmp, DataDirs::new(data))
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn the_core_is_where_its_record_says_and_nowhere_when_it_has_none() {
        let (_tmp, dirs) = dirs();
        assert_eq!(core(&dirs), Err(Check::Missing));
        write(&dirs.core_record(), r#"{"pid":1,"endpoint":"/run/user/1/sidevoice-ab/local.sock","token":"x"}"#);
        assert_eq!(core(&dirs), Ok(PathBuf::from("/run/user/1/sidevoice-ab/local.sock")));
        for bad in ["{}", r#"{"endpoint":"relative.sock"}"#, r#"{"endpoint":3}"#, "nope"] {
            write(&dirs.core_record(), bad);
            assert!(matches!(core(&dirs), Err(Check::Unsafe(r)) if r.key == "endpoint.record-invalid"), "{bad}");
        }
    }

    #[test]
    fn the_connector_is_where_its_record_says_while_its_pid_is_alive() {
        let (_tmp, dirs) = dirs();
        std::fs::set_permissions(&dirs.data, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(connector(&dirs), Err(Check::Missing));
        let me = std::process::id();
        write(
            &dirs.connector_record(),
            &format!(r#"{{"pid":{me},"endpoint":"/run/user/1/sidevoice-ab/connector.sock"}}"#),
        );
        assert_eq!(connector(&dirs), Ok(PathBuf::from("/run/user/1/sidevoice-ab/connector.sock")));
        // A crashed connector leaves its record: a pid nobody has is stale.
        let mut gone = std::process::Command::new("true").spawn().unwrap();
        let dead = gone.id();
        gone.wait().unwrap();
        write(&dirs.connector_record(), &format!(r#"{{"pid":{dead},"endpoint":"/run/user/1/x/connector.sock"}}"#));
        assert_eq!(connector(&dirs), Err(Check::Missing));
        write(&dirs.connector_record(), r#"{"endpoint":"/run/user/1/x/connector.sock"}"#);
        assert!(matches!(connector(&dirs), Err(Check::Unsafe(_))));
    }
}
