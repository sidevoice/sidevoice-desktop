//! What the app owes the core it installed: a copy of its switch (docs/CRASH_REPORTS.md → "Copied to the core the app installed").
//!
//! One small file of its own, `crash-core-copy.json`, in the app's directory, apart from the setting (`settings.rs`) so that a
//! setting file that cannot be read never decides what is known here:
//!
//! - **No file**: the app did not install a core on this computer. Its switch is the core's own business; nothing is copied.
//! - **`{"owed": true}`**: the app installed a core and its switch has not been delivered since the app last owed it: when the app
//!   installed the core, when the person changed the switch, or when a copy failed. It stays owed until the core answers a copy
//!   with a 200 ([`CoreCopy::delivered`]); every time the local host sees the core reachable while it is owed, the app tries once.
//! - **`{"owed": false}`**: the last copy was delivered. A change made on the core after that (its CLI) stands: the app copies
//!   again only when it owes it again, that is, when the person next changes the app's switch.
//! - **A file that is there but cannot be read or understood**: unknown. Nothing is copied and nothing is written over it (a
//!   downgraded value would clear a marker that may have been there); only an install by the app, which knows what it did,
//!   replaces it.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::settings::{read_small, write_atomic};
use crate::error::SharedError;

pub const FILE: &str = "crash-core-copy.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    owed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// No file: the app did not install a core.
    NotInstalled,
    /// The app installed a core; whether a copy of its switch is owed.
    Installed { owed: bool },
    /// The file cannot be read: not known.
    Unknown,
}

pub struct CoreCopy {
    path: PathBuf,
    state: State,
}

impl CoreCopy {
    /// What `dir` holds.
    pub fn load(dir: &Path) -> CoreCopy {
        let path = dir.join(FILE);
        let state = match read_small(&path) {
            Ok(None) => State::NotInstalled,
            Ok(Some(bytes)) => match serde_json::from_slice::<Stored>(&bytes) {
                Ok(stored) => State::Installed { owed: stored.owed },
                Err(_) => unreadable(std::io::ErrorKind::InvalidData),
            },
            Err(kind) => unreadable(kind),
        };
        CoreCopy { path, state }
    }

    /// No place to keep anything (the app's directory could not be had): nothing is known, nothing can be written.
    pub fn unavailable() -> CoreCopy {
        CoreCopy { path: PathBuf::new(), state: State::Unknown }
    }

    /// Whether a copy of the switch is owed to a core the app installed.
    pub fn owed(&self) -> bool {
        self.state == State::Installed { owed: true }
    }

    /// The app installed a core: a copy is owed, whatever the file said before (an install is not a downgrade of anything).
    /// What is known in memory is the new state even when the file cannot be written: the error says it was not kept.
    pub fn installed(&mut self) -> io::Result<()> {
        self.state = State::Installed { owed: true };
        self.write(true)
    }

    /// The person changed the switch: a copy is owed, if the app installed a core. A file that cannot be read is not written.
    pub fn owe(&mut self) -> io::Result<()> {
        match self.state {
            State::Installed { .. } => {
                self.state = State::Installed { owed: true };
                self.write(true)
            }
            State::NotInstalled | State::Unknown => Ok(()),
        }
    }

    /// The core answered a copy with a 200: nothing is owed. Kept in memory even when the file cannot be written (the file then
    /// still says owed, and one more copy goes the next time the core is seen).
    pub fn delivered(&mut self) -> io::Result<()> {
        match self.state {
            State::Installed { .. } => {
                self.state = State::Installed { owed: false };
                self.write(false)
            }
            State::NotInstalled | State::Unknown => Ok(()),
        }
    }

    fn write(&self, owed: bool) -> io::Result<()> {
        write_atomic(&self.path, &serde_json::to_vec(&Stored { owed }).map_err(io::Error::other)?)
    }
}

fn unreadable(kind: io::ErrorKind) -> State {
    tracing::warn!(
        error.type = SharedError::CrashSettingsUnreadable.code(),
        error.kind = ?kind,
        "the record of whether the app installed the core could not be read; the crash report switch is not copied to it"
    );
    State::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_disk(dir: &Path) -> Option<String> {
        std::fs::read_to_string(dir.join(FILE)).ok()
    }

    #[test]
    fn no_file_is_no_core_of_the_apps_and_nothing_is_written_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut copy = CoreCopy::load(dir.path());
        assert!(!copy.owed());
        copy.owe().unwrap();
        copy.delivered().unwrap();
        assert!(!copy.owed());
        assert_eq!(on_disk(dir.path()), None, "a core the app did not install is never marked");
    }

    #[test]
    fn an_install_owes_a_copy_until_a_200_and_a_change_owes_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut copy = CoreCopy::load(dir.path());
        copy.installed().unwrap();
        assert!(copy.owed());
        assert_eq!(on_disk(dir.path()).as_deref(), Some(r#"{"owed":true}"#));
        assert!(CoreCopy::load(dir.path()).owed(), "kept across runs: a copy that never got through is tried again");
        copy.delivered().unwrap();
        assert!(!copy.owed());
        assert!(!CoreCopy::load(dir.path()).owed(), "delivered: nothing is re-sent at the next start");
        assert_eq!(on_disk(dir.path()).as_deref(), Some(r#"{"owed":false}"#));
        copy.owe().unwrap();
        assert!(copy.owed());
        assert!(CoreCopy::load(dir.path()).owed());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_unknown_and_never_written_over_except_by_an_install() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        for broken in
            [b"not json".to_vec(), vec![b' '; 5000], br#"{"owed":"yes"}"#.to_vec(), br#"{"owed":true,"x":1}"#.to_vec()]
        {
            std::fs::write(&file, &broken).unwrap();
            let mut copy = CoreCopy::load(dir.path());
            assert!(!copy.owed(), "unknown is not owed");
            copy.owe().unwrap();
            copy.delivered().unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), broken, "no downgraded value is written back");
        }
        // A directory where the file should be cannot be read either.
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let mut copy = CoreCopy::load(dir.path());
        copy.owe().unwrap();
        assert!(!copy.owed());
        std::fs::remove_dir(&file).unwrap();
        // The app installing a core knows what it did: that replaces what was there.
        std::fs::write(&file, b"not json").unwrap();
        let mut copy = CoreCopy::load(dir.path());
        copy.installed().unwrap();
        assert!(copy.owed());
        assert_eq!(on_disk(dir.path()).as_deref(), Some(r#"{"owed":true}"#));
    }

    #[test]
    fn a_failed_write_keeps_what_the_run_knows_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the temporary file goes: nothing can be written.
        std::fs::create_dir(dir.path().join("crash-core-copy.json.tmp")).unwrap();
        let mut copy = CoreCopy::load(dir.path());
        assert!(copy.installed().is_err());
        assert!(copy.owed(), "owed in this run, whatever the disk says");
        let none = CoreCopy::unavailable();
        assert!(!none.owed());
    }
}
