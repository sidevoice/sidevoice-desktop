//! What the app checks before it trusts a file or a socket of the local host (SEAMS §5): the core's directory is
//! this user's and private (`identity.unsafe-directory`), the connector's directory and `install.json` are this
//! user's and nobody else can write them, and whoever answers on a socket runs as this user (`peer.uid-mismatch`).
//!
//! A path that is not there is not unsafe: it is [`Check::Missing`], and the caller treats the host as absent.

use crate::Refusal;
use std::io;
use std::os::unix::fs::MetadataExt;
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

fn unsafe_directory(path: &Path, why: &str) -> Check {
    Check::Unsafe(Refusal::new(
        "identity.unsafe-directory",
        format!("{} is not a private directory of this user ({why}); the app does not connect to it.", path.display()),
    ))
}

/// The core's directory: a real directory (not a link), owned by this user, mode & 077 = 0.
pub fn private_dir(path: &Path) -> Result<(), Check> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Check::Missing),
        Err(e) => return Err(unsafe_directory(path, &e.to_string())),
    };
    if !meta.file_type().is_dir() {
        return Err(unsafe_directory(path, "not a directory"));
    }
    if meta.uid() != uid() {
        return Err(unsafe_directory(path, &format!("owned by uid {}", meta.uid())));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(unsafe_directory(path, &format!("mode {:o}", meta.mode() & 0o777)));
    }
    Ok(())
}

/// The connector's directory or a file in it the app acts on (`install.json`): owned by this user, and neither its
/// group nor others can write it. Not a link.
pub fn owned_not_writable(path: &Path) -> Result<(), Check> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Check::Missing),
        Err(e) => return Err(unsafe_file(path, &e.to_string())),
    };
    if meta.file_type().is_symlink() {
        return Err(unsafe_file(path, "a symbolic link"));
    }
    if meta.uid() != uid() {
        return Err(unsafe_file(path, &format!("owned by uid {}", meta.uid())));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(unsafe_file(path, &format!("mode {:o}", meta.mode() & 0o777)));
    }
    Ok(())
}

fn unsafe_file(path: &Path, why: &str) -> Check {
    Check::Unsafe(Refusal::new(
        "install.unsafe",
        format!("{} may be changed by another user ({why}); the app does not run what it names.", path.display()),
    ))
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
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_private_directory_passes_and_anything_looser_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("core");
        assert_eq!(private_dir(&dir), Err(Check::Missing));
        std::fs::create_dir(&dir).unwrap();
        chmod(&dir, 0o700);
        assert_eq!(private_dir(&dir), Ok(()));
        for mode in [0o750, 0o705, 0o770, 0o777, 0o710] {
            chmod(&dir, mode);
            let Err(Check::Unsafe(refusal)) = private_dir(&dir) else { panic!("mode {mode:o} passed") };
            assert_eq!(refusal.key, "identity.unsafe-directory");
        }
    }

    #[test]
    fn a_link_or_a_file_is_not_the_core_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        chmod(&real, 0o700);
        let link = tmp.path().join("core");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(private_dir(&link), Err(Check::Unsafe(_))));
        let file = tmp.path().join("file");
        std::fs::write(&file, b"").unwrap();
        chmod(&file, 0o600);
        assert!(matches!(private_dir(&file), Err(Check::Unsafe(_))));
    }

    #[test]
    fn install_record_must_not_be_writable_by_others() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("install.json");
        assert_eq!(owned_not_writable(&file), Err(Check::Missing));
        std::fs::write(&file, b"{}").unwrap();
        for mode in [0o600, 0o644, 0o640, 0o700, 0o755] {
            chmod(&file, mode);
            assert_eq!(owned_not_writable(&file), Ok(()), "mode {mode:o}");
        }
        for mode in [0o620, 0o602, 0o666, 0o777] {
            chmod(&file, mode);
            let Err(Check::Unsafe(refusal)) = owned_not_writable(&file) else { panic!("mode {mode:o} passed") };
            assert_eq!(refusal.key, "install.unsafe");
        }
    }

    #[test]
    fn the_peer_of_our_own_socket_is_us() {
        let tmp = tempfile::tempdir().unwrap();
        chmod(tmp.path(), 0o700);
        let path = tmp.path().join("s.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let stream = connect(tmp.path(), &path, private_dir).unwrap();
        assert_eq!(peer_uid(&stream).unwrap(), uid());
        // Nobody listening: absent, not unsafe.
        assert_eq!(connect(tmp.path(), &tmp.path().join("none.sock"), private_dir).err(), Some(Check::Missing));
        // A loose directory: refused before any connect.
        chmod(tmp.path(), 0o755);
        assert!(matches!(connect(tmp.path(), &path, private_dir), Err(Check::Unsafe(_))));
        chmod(tmp.path(), 0o700);
    }
}
