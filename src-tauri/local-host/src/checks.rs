//! What the app checks before it trusts a file or a socket of the local host (SEAMS §5): the core's directory is
//! this user's and private (`identity.unsafe-directory`), the connector's directory and `install.json` are this
//! user's and nobody else can write them, and whoever answers on a socket runs as this user (`peer.uid-mismatch`).
//!
//! A path that is not there is not unsafe: it is [`Check::Missing`], and the caller treats the host as absent.

use crate::platform::ipc::{self, Peer, Stream};
use crate::Refusal;
use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
use sidevoice_platform::ipc::Endpoint;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Missing,
    Unsafe(Refusal),
}

impl From<Check> for Refusal {
    fn from(check: Check) -> Refusal {
        match check {
            Check::Missing => Refusal::new("host.absent", "There is no local host on this computer."),
            Check::Unsafe(refusal) => refusal,
        }
    }
}

/// Preserve the host's absent/refused distinction and the caller's stable refusal key.
pub(crate) fn storage_error(path: &Path, key: &str, error: io::Error) -> Check {
    if error.kind() == io::ErrorKind::NotFound {
        Check::Missing
    } else {
        Check::Unsafe(Refusal::new(
            key,
            format!("{} cannot be trusted ({error}); use an ordinary directory owned by this user and not writable by others.", path.display()),
        ))
    }
}

/// The core's existing directory, checked with its ancestry; discovery never creates it.
pub fn private_dir(path: &Path) -> Result<(), Check> {
    HeldDirectory::open(path, Access::Private)
        .map(|_| ())
        .map_err(|e| storage_error(path, "identity.unsafe-directory", e))
}

/// The connector's existing directory, readable by others but never writable by them.
pub fn owned_not_writable(path: &Path) -> Result<(), Check> {
    HeldDirectory::open(path, Access::NotWritableByOthers)
        .map(|_| ())
        .map_err(|e| storage_error(path, "install.unsafe", e))
}

/// Connects to `endpoint`, whose directory is `dir`, after checking `dir` with `dir_check`, and refuses a peer that is
/// not this user. An endpoint nobody listens on (absent, refused) is [`Check::Missing`].
pub fn connect(dir: &Path, endpoint: &Endpoint, dir_check: fn(&Path) -> Result<(), Check>) -> Result<Stream, Check> {
    dir_check(dir)?;
    let stream = match ipc::connect(endpoint) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            return Err(Check::Missing)
        }
        Err(e) => {
            return Err(Check::Unsafe(Refusal::new("socket.unreachable", format!("{endpoint} cannot be reached: {e}"))))
        }
    };
    let peer = stream.peer().map_err(|e| {
        Check::Unsafe(Refusal::new("peer.uid-mismatch", format!("Who answers on {endpoint} is unknown: {e}")))
    })?;
    if let Peer::Other(peer) = peer {
        return Err(Check::Unsafe(Refusal::new(
            "peer.uid-mismatch",
            format!("{endpoint} is served by {peer}, not this user; the app does not talk to it."),
        )));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_test_support::platform::storage::{StorageFixture, StorageFixtures};
    use sidevoice_platform::fs::Directory;

    #[test]
    fn a_private_directory_passes_and_anything_looser_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("core");
        assert_eq!(private_dir(&dir), Err(Check::Missing));
        std::fs::create_dir(&dir).unwrap();
        StorageFixtures::private(&dir);
        assert_eq!(private_dir(&dir), Ok(()));
        StorageFixtures::private_directory_violations(&dir, || {
            let Err(Check::Unsafe(refusal)) = private_dir(&dir) else { panic!("insecure access passed") };
            assert_eq!(refusal.key, "identity.unsafe-directory");
        });
    }

    #[test]
    fn a_link_of_this_users_leads_to_the_core_directory_but_a_file_is_not_one() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        StorageFixtures::private(&real);
        let link = tmp.path().join("core");
        StorageFixtures::symlink(&real, &link);
        assert_eq!(private_dir(&link), Ok(()));
        let file = tmp.path().join("file");
        std::fs::write(&file, b"").unwrap();
        StorageFixtures::private(&file);
        assert!(matches!(private_dir(&file), Err(Check::Unsafe(_))));
    }

    #[test]
    fn install_record_must_not_be_writable_by_others() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("install.json");
        let dir = HeldDirectory::open(tmp.path(), Access::NotWritableByOthers).unwrap();
        let check = || {
            dir.read("install.json".as_ref(), Access::NotWritableByOthers, 1024)
                .map_err(|e| storage_error(&file, "install.unsafe", e))?
                .ok_or(Check::Missing)
        };
        assert_eq!(check(), Err(Check::Missing));
        std::fs::write(&file, b"{}").unwrap();
        StorageFixtures::record_access_cases(&file, |trusted| {
            if trusted {
                assert_eq!(check(), Ok(b"{}".to_vec()));
            } else {
                let Err(Check::Unsafe(refusal)) = check() else { panic!("writable record passed") };
                assert_eq!(refusal.key, "install.unsafe");
            }
        });
    }

    #[test]
    fn the_peer_of_our_own_socket_is_us() {
        let tmp = tempfile::tempdir().unwrap();
        StorageFixtures::private(tmp.path());
        let endpoint = ipc::scratch_endpoint(tmp.path(), "s");
        let _listener = ipc::listen(&endpoint).unwrap();
        let stream = connect(tmp.path(), &endpoint, private_dir).unwrap();
        assert_eq!(stream.peer().unwrap(), Peer::ThisUser);
        // Nobody listening: absent, not unsafe.
        let none = ipc::scratch_endpoint(tmp.path(), "none");
        assert_eq!(connect(tmp.path(), &none, private_dir).err(), Some(Check::Missing));
        // A loose directory: refused before any connect.
        StorageFixtures::readable(tmp.path());
        assert!(matches!(connect(tmp.path(), &endpoint, private_dir), Err(Check::Unsafe(_))));
        StorageFixtures::private(tmp.path());
    }
}
