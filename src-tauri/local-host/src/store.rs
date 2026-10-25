//! `local-host.json` in the app's config directory (SEAMS §5): the app's pairing with this computer's core, the only
//! copy of its device token. Written 0600, atomically; one writer, the app's native side. A file that is missing or
//! unreadable is no pairing: the app pairs again (which revokes the previous local device in the core).

use crate::checks::Check;
use crate::platform::storage::{Access, Directory, HeldDirectory, Storage, TrustedStorage};
use crate::Refusal;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "local-host.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    /// The core's fingerprint when the app paired.
    pub fp: String,
    /// The key the core proves itself with (SPKI DER, base64), pinned.
    pub public_key: String,
    pub device_id: String,
    /// The device token: sent only over the core's socket, never to the page.
    pub token: String,
    /// The machine's name, as the core calls itself.
    pub host: String,
    /// ISO-8601, UTC.
    pub paired_at: String,
}

impl Pairing {
    /// The token, kept out of `{:?}`.
    pub fn redacted(&self) -> String {
        format!(
            "{{fp: {}, device_id: {}, host: {}, paired_at: {}}}",
            self.fp, self.device_id, self.host, self.paired_at
        )
    }
}

/// The app's config directory, checked as opened: its ancestry safe from other users, itself this user's and not
/// writable by others (review R1-c #5).
fn open(dir: &Path) -> Result<HeldDirectory, Check> {
    Storage::open(dir, Access::NotWritableByOthers).map_err(|e| crate::checks::storage_error(dir, "app.storage", e))
}

/// The stored pairing, read through the checked directory: a regular private file (0600), not a link.
pub fn load(dir: &Path) -> Option<Pairing> {
    let bytes = open(dir).ok()?.read(FILE_NAME.as_ref(), Access::Private, 64 * 1024).ok()??;
    serde_json::from_slice(&bytes).ok()
}

/// Writes the pairing next to the file, 0600, synced, then renames it over and syncs the directory: a crash leaves
/// the old file or the new. A directory whose ancestry others can change is refused.
pub fn save(dir: &Path, pairing: &Pairing) -> io::Result<()> {
    // Existing readable app parents are valid; never tighten their permissions. Only a genuinely missing
    // directory is created, with the shared private creation/check policy.
    let opened = match open(dir) {
        Err(Check::Missing) => {
            Storage::ensure_private_dir(dir)?;
            open(dir)
        }
        opened => opened,
    };
    let dir = opened.map_err(|check| io::Error::other(Refusal::from(check)))?;
    dir.write(FILE_NAME.as_ref(), &serde_json::to_vec_pretty(pairing).map_err(io::Error::other)?)
}

pub fn remove(dir: &Path) -> io::Result<()> {
    match open(dir) {
        Ok(dir) => dir.remove(FILE_NAME.as_ref()),
        Err(Check::Missing) => Ok(()),
        Err(check) => Err(io::Error::other(Refusal::from(check))),
    }
}

/// Now, as ISO-8601 UTC to the second (`2026-10-01T12:00:00Z`).
pub fn now_iso() -> String {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    iso(seconds)
}

fn iso(seconds: u64) -> String {
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Days since the epoch to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_test_support::platform::storage::{Others, StorageFixture, StorageFixtures};
    use std::fs;

    fn pairing() -> Pairing {
        Pairing {
            fp: "fp".into(),
            public_key: "key".into(),
            device_id: "dev".into(),
            token: "secret-token".into(),
            host: "MacBook".into(),
            paired_at: now_iso(),
        }
    }

    #[test]
    fn saved_0600_and_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("config");
        assert_eq!(load(&dir), None);
        save(&dir, &pairing()).unwrap();
        assert_eq!(load(&dir), Some(pairing()).map(|p| Pairing { paired_at: load(&dir).unwrap().paired_at, ..p }));
        StorageFixtures::assert_private(&dir.join(FILE_NAME));
        StorageFixtures::assert_private(&dir);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "nothing staged is left");
        assert!(!pairing().redacted().contains("secret-token"));
        remove(&dir).unwrap();
        remove(&dir).unwrap();
        assert_eq!(load(&dir), None);
    }

    #[test]
    fn a_corrupt_file_is_no_pairing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(FILE_NAME), b"{not json").unwrap();
        assert_eq!(load(tmp.path()), None);
    }

    #[test]
    fn a_failed_write_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("config");
        fs::write(&blocked, b"a file where the directory should be").unwrap();
        assert!(save(&blocked, &pairing()).is_err());
    }

    #[test]
    fn a_config_directory_others_can_change_holds_no_credential() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        fs::create_dir(&shared).unwrap();
        StorageFixtures::writable(&shared);
        let dir = shared.join("dev.sidevoice.desktop");
        assert!(save(&dir, &pairing()).is_err(), "refused under a directory others can rename in");
        assert_eq!(load(&dir), None);
        StorageFixtures::private(&shared);
        StorageFixtures::readable(&shared);
        save(&dir, &pairing()).unwrap();
        assert!(load(&dir).is_some());
        // A credential others can read is not one.
        StorageFixtures::readable(&dir.join(FILE_NAME));
        assert_eq!(load(&dir), None);
    }

    #[test]
    fn existing_readable_app_directory_and_pairing_are_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("config");
        fs::create_dir(&dir).unwrap();
        StorageFixtures::readable(&dir);
        let old = pairing();
        let bytes = serde_json::to_vec_pretty(&old).unwrap();
        fs::write(dir.join(FILE_NAME), &bytes).unwrap();
        StorageFixtures::private(&dir.join(FILE_NAME));
        assert_eq!(load(&dir), Some(old.clone()));
        assert_eq!(fs::read(dir.join(FILE_NAME)).unwrap(), bytes);
        save(&dir, &old).unwrap();
        assert_eq!(StorageFixtures::others(&dir), Others::Read);
        assert_eq!(load(&dir), Some(old));
    }

    #[test]
    fn unsafe_existing_records_are_never_repaired_or_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(FILE_NAME);
        let other = tmp.path().join("other.json");
        save(tmp.path(), &pairing()).unwrap();
        fs::hard_link(&path, &other).unwrap();
        let original = fs::read(&path).unwrap();
        assert_eq!(load(tmp.path()), None);
        assert!(save(tmp.path(), &pairing()).is_err());
        assert!(remove(tmp.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&other).unwrap(), original);
        fs::remove_file(&path).unwrap();
        StorageFixtures::symlink(&other, &path);
        assert_eq!(load(tmp.path()), None);
        assert!(save(tmp.path(), &pairing()).is_err());
        assert!(remove(tmp.path()).is_err());
        assert_eq!(fs::read(&other).unwrap(), original);
    }

    #[test]
    fn discovery_and_remove_do_not_create_an_absent_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("missing");
        assert_eq!(load(&dir), None);
        remove(&dir).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_759_320_000), "2025-10-01T12:00:00Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
    }
}
