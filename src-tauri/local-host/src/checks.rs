//! What the app checks before it trusts a file or a socket of the local host (SEAMS §5): the core's directory is
//! this user's and private (`identity.unsafe-directory`), the connector's directory and `install.json` are this
//! user's and nobody else can write them, and whoever answers on a socket runs as this user (`peer.uid-mismatch`).
//!
//! A path that is not there is not unsafe: it is [`Check::Missing`], and the caller treats the host as absent.

use crate::Refusal;
use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

/// The effective uid of this process: the OS user the trust boundary is.
pub fn uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

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

/// The uid of the process at the other end of a connected Unix socket.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for writes of the sizes passed; the fd is open for the call.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

/// The uid of the process at the other end of a connected Unix socket.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `uid` and `gid` are valid for writes; the fd is open for the call.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

/// Connects to `socket` in `dir` after checking `dir` with `dir_check`, and refuses a peer that is not this user.
/// A socket nobody listens on (absent, refused) is [`Check::Missing`].
pub fn connect(dir: &Path, socket: &Path, dir_check: fn(&Path) -> Result<(), Check>) -> Result<UnixStream, Check> {
    dir_check(dir)?;
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
            return Err(Check::Missing)
        }
        Err(e) => {
            return Err(Check::Unsafe(Refusal::new(
                "socket.unreachable",
                format!("{} cannot be reached: {e}", socket.display()),
            )))
        }
    };
    let peer = peer_uid(&stream).map_err(|e| {
        Check::Unsafe(Refusal::new("peer.uid-mismatch", format!("Who answers on {} is unknown: {e}", socket.display())))
    })?;
    if peer != uid() {
        return Err(Check::Unsafe(Refusal::new(
            "peer.uid-mismatch",
            format!("{} is served by uid {peer}, not this user; the app does not talk to it.", socket.display()),
        )));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_test_support::platform::storage::{StorageFixture, StorageFixtures};
    use sidevoice_platform::fs::Directory;
    use std::os::unix::net::UnixListener;

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
    fn a_link_or_a_file_is_not_the_core_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        StorageFixtures::private(&real);
        let link = tmp.path().join("core");
        StorageFixtures::symlink(&real, &link);
        assert!(matches!(private_dir(&link), Err(Check::Unsafe(_))));
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
        let path = tmp.path().join("s.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let stream = connect(tmp.path(), &path, private_dir).unwrap();
        assert_eq!(peer_uid(&stream).unwrap(), uid());
        // Nobody listening: absent, not unsafe.
        assert_eq!(connect(tmp.path(), &tmp.path().join("none.sock"), private_dir).err(), Some(Check::Missing));
        // A loose directory: refused before any connect.
        StorageFixtures::readable(tmp.path());
        assert!(matches!(connect(tmp.path(), &path, private_dir), Err(Check::Unsafe(_))));
        StorageFixtures::private(tmp.path());
    }
}
