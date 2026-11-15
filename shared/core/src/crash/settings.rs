//! The setting and the installation id, in `crash-reports.json` in the app's own directory. A file that is missing or
//! unreadable is the default (`auto`, no id yet): the app never asks, and never fails because of it.
//!
//! The installation id is random (never derived from hardware or an account), pseudonymous, replaced after a year, and
//! removed when the person chooses `never`: nothing keeps identifying the installation once it stops reporting.

use super::Mode;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE: &str = "crash-reports.json";
/// How long an installation id lasts.
pub const ID_LIFETIME_SECS: u64 = 365 * 24 * 60 * 60;
/// The most the file may hold when read.
const LIMIT: u64 = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Install {
    id: String,
    /// Unix seconds.
    created: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    mode: Mode,
    #[serde(default)]
    install: Option<Install>,
}

pub struct Settings {
    path: PathBuf,
    stored: Stored,
}

/// 128 random bits as 32 hex digits, from the OS.
fn random_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| std::io::Error::other("no randomness"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn plain_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Settings {
    /// What `dir` holds, or the default.
    pub fn load(dir: &Path) -> Settings {
        let path = dir.join(FILE);
        let stored = std::fs::File::open(&path)
            .ok()
            .and_then(|file| {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut std::io::Read::take(file, LIMIT + 1), &mut bytes).ok()?;
                (bytes.len() as u64 <= LIMIT).then_some(bytes)
            })
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .map(|mut stored| {
                if stored.install.as_ref().is_some_and(|install| !plain_id(&install.id)) {
                    stored.install = None;
                }
                stored
            })
            .unwrap_or_default();
        Settings { path, stored }
    }

    pub fn mode(&self) -> Mode {
        self.stored.mode
    }

    /// Keeps `mode`. `never` also forgets the installation id. Nothing changes when the file cannot be written.
    pub fn set_mode(&mut self, mode: Mode) -> std::io::Result<()> {
        let mut next = self.stored.clone();
        next.mode = mode;
        if mode == Mode::Never {
            next.install = None;
        }
        self.write(&next)?;
        self.stored = next;
        Ok(())
    }

    /// The installation id as of `now` (Unix seconds): made on first use and again when a year old.
    pub fn install_id(&mut self, now: u64) -> std::io::Result<String> {
        if let Some(install) = &self.stored.install {
            if now.saturating_sub(install.created) < ID_LIFETIME_SECS && install.created <= now + ID_LIFETIME_SECS {
                return Ok(install.id.clone());
            }
        }
        let mut next = self.stored.clone();
        let id = random_id()?;
        next.install = Some(Install { id: id.clone(), created: now });
        self.write(&next)?;
        self.stored = next;
        Ok(id)
    }

    /// Atomically: a temporary file beside it, then a rename.
    fn write(&self, stored: &Stored) -> std::io::Result<()> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let temporary = self.path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&serde_json::to_vec(stored).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_auto_and_survives_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Auto);
        std::fs::write(dir.path().join(FILE), b"not json").unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Auto);
        std::fs::write(dir.path().join(FILE), vec![b' '; 5000]).unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Auto);
    }

    #[test]
    fn the_mode_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path());
        settings.set_mode(Mode::Never).unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Never);
        settings.set_mode(Mode::Auto).unwrap();
        assert_eq!(Settings::load(dir.path()).mode(), Mode::Auto);
    }

    #[test]
    fn the_install_id_is_random_stable_for_a_year_then_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path());
        let first = settings.install_id(1_000).unwrap();
        assert!(plain_id(&first));
        assert_eq!(settings.install_id(2_000).unwrap(), first);
        assert_eq!(Settings::load(dir.path()).install_id(3_000).unwrap(), first, "kept on disk");
        let second = settings.install_id(1_000 + ID_LIFETIME_SECS).unwrap();
        assert_ne!(second, first, "rotated after a year");
        assert_ne!(Settings::load(dir.path()).install_id(1_000 + ID_LIFETIME_SECS + 5).unwrap(), first);
    }

    #[test]
    fn never_forgets_the_install_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::load(dir.path());
        let first = settings.install_id(10).unwrap();
        settings.set_mode(Mode::Never).unwrap();
        let text = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!text.contains(&first), "{text}");
        settings.set_mode(Mode::Auto).unwrap();
        assert_ne!(settings.install_id(11).unwrap(), first, "a new id after never");
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        let mut settings = Settings::load(&blocker);
        assert!(settings.set_mode(Mode::Never).is_err());
        assert_eq!(settings.mode(), Mode::Auto);
    }
}
