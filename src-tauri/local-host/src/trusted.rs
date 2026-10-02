//! Files the app acts on, bound to what was checked (review R1-c #5): a path whose ancestry another user can
//! change is refused, and a file is read or written through a directory descriptor that was opened and checked
//! itself, opened without following links and checked with `fstat` — never checked by path and reopened by path.
//!
//! - [`ancestry`]: every directory above a path (links resolved) is root's or this user's, and nobody else can
//!   rename in it (not group- or other-writable, unless sticky, as `/tmp`). `~/.sidevoice` and the app's config
//!   directory pass in the ordinary layout; a `SIDEVOICE_DATA_DIR` under a directory others can write does not.
//! - [`executable`]: a program the app runs (`install.json` `command`): its ancestry, and the file itself root's or
//!   this user's and not writable by others.
//! - [`Dir`]: a checked directory descriptor; reads, links, child directories and writes stay bound to it.

use crate::checks::{uid, Check};
use crate::Refusal;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

fn unsafe_path(key: &str, path: &Path, why: &str) -> Check {
    Check::Unsafe(Refusal::new(
        key,
        format!("{} may be changed by another user ({why}); the app does not act on it.", path.display()),
    ))
}

fn changeable_by_others(meta: &std::fs::Metadata) -> bool {
    let owner_ok = meta.uid() == 0 || meta.uid() == uid();
    let sticky = meta.mode() & 0o1000 != 0;
    !owner_ok || (meta.mode() & 0o022 != 0 && !sticky)
}

/// `path` with links resolved, once every directory above it is safe from other users (`key` names the refusal).
/// A path that does not exist is [`Check::Missing`].
pub fn ancestry(path: &Path, key: &str) -> Result<PathBuf, Check> {
    let resolved = match std::fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Check::Missing),
        Err(e) => return Err(unsafe_path(key, path, &e.to_string())),
    };
    for above in resolved.ancestors().skip(1) {
        let meta = std::fs::metadata(above).map_err(|e| unsafe_path(key, above, &e.to_string()))?;
        if changeable_by_others(&meta) {
            return Err(unsafe_path(key, above, &format!("uid {} mode {:o}", meta.uid(), meta.mode() & 0o7777)));
        }
    }
    Ok(resolved)
}

/// A program the app runs: safe ancestry, a regular file of root's or this user's that others cannot write. Returns the
/// path with every link resolved — the one that was checked, and the one to run: a link on the way (a system one
/// such as `/usr/bin/node`, or one in a directory others can change) is never followed again after the check.
pub fn executable(path: &Path) -> Result<PathBuf, Check> {
    let resolved = ancestry(path, "install.unsafe")?;
    let meta = std::fs::metadata(&resolved).map_err(|e| unsafe_path("install.unsafe", path, &e.to_string()))?;
    if !meta.is_file() || changeable_by_others(&meta) || meta.mode() & 0o1000 != 0 {
        return Err(unsafe_path(
            "install.unsafe",
            path,
            &format!("uid {} mode {:o}", meta.uid(), meta.mode() & 0o7777),
        ));
    }
    Ok(resolved)
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

// `mode_t` is u32 on Linux and u16 on macOS: the casts are needed on one of them.
#[allow(clippy::unnecessary_cast)]
fn check_fd(fd: &OwnedFd, directory: bool, mask: u32, key: &str, path: &Path) -> Result<(), Check> {
    // SAFETY: an all-zero `stat` is a valid value to be overwritten; the fd is open.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(unsafe_path(key, path, &io::Error::last_os_error().to_string()));
    }
    let kind = stat.st_mode as u32 & libc::S_IFMT as u32;
    let wanted = if directory { libc::S_IFDIR } else { libc::S_IFREG } as u32;
    let mode = stat.st_mode as u32 & 0o7777;
    if kind != wanted || stat.st_uid != uid() || mode & mask != 0 {
        return Err(unsafe_path(key, path, &format!("uid {} mode {mode:o}", stat.st_uid)));
    }
    Ok(())
}

fn open_error(e: io::Error, key: &str, path: &Path) -> Check {
    match e.raw_os_error() {
        Some(libc::ENOENT) => Check::Missing,
        Some(libc::ELOOP) | Some(libc::ENOTDIR) => unsafe_path(key, path, "a symbolic link or not a directory"),
        _ => unsafe_path(key, path, &e.to_string()),
    }
}

/// A directory opened without following a link at its last step, and checked as opened: this user's, and with no
/// permission bit in `mask`.
#[derive(Debug)]
pub struct Dir {
    fd: OwnedFd,
    path: PathBuf,
    key: String,
}

impl Dir {
    /// Opens `path` after checking its ancestry ([`ancestry`]); `key` names the refusal.
    pub fn open(path: &Path, mask: u32, key: &str) -> Result<Dir, Check> {
        let resolved = ancestry(path, key)?;
        let c = c_path(&resolved).map_err(|e| unsafe_path(key, path, &e.to_string()))?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: `c` is a NUL-terminated path; the returned fd, if any, is owned here.
        let raw = unsafe { libc::open(c.as_ptr(), flags) };
        if raw < 0 {
            return Err(open_error(io::Error::last_os_error(), key, path));
        }
        // SAFETY: `raw` is a freshly opened descriptor nobody else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        check_fd(&fd, true, mask, key, path)?;
        Ok(Dir { fd, path: resolved, key: key.to_string() })
    }

    /// The directory as checked: every link resolved.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn openat(&self, name: &str, flags: libc::c_int, mode: libc::mode_t) -> io::Result<OwnedFd> {
        let c = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: `c` is NUL-terminated, `self.fd` is an open directory; the returned fd is owned here.
        let raw = unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                c.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode as libc::c_uint,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: as above.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    /// Opens a checked child directory relative to this descriptor, without following the final component.
    pub fn open_child(&self, name: &str, mask: u32) -> Result<Dir, Check> {
        let path = self.path.join(name);
        if !one_component(name) {
            return Err(unsafe_path(&self.key, &path, "the child name is not one directory component"));
        }
        let fd =
            self.openat(name, libc::O_RDONLY | libc::O_DIRECTORY, 0).map_err(|e| open_error(e, &self.key, &path))?;
        check_fd(&fd, true, mask, &self.key, &path)?;
        Ok(Dir { fd, path, key: self.key.clone() })
    }

    /// Reads one symbolic link by name from this checked directory descriptor. Its target is returned verbatim;
    /// callers must validate the target before opening anything it names.
    pub fn read_link(&self, name: &str) -> Result<PathBuf, Check> {
        let path = self.path.join(name);
        if !one_component(name) {
            return Err(unsafe_path(&self.key, &path, "the link name is not one directory component"));
        }
        let c_name = CString::new(name).map_err(|e| unsafe_path(&self.key, &path, &e.to_string()))?;
        let mut bytes = vec![0u8; 4096];
        // SAFETY: `c_name` is NUL-terminated and `bytes` is a writable buffer for the supplied size.
        let count = unsafe {
            libc::readlinkat(self.fd.as_raw_fd(), c_name.as_ptr(), bytes.as_mut_ptr() as *mut libc::c_char, bytes.len())
        };
        if count < 0 {
            return Err(open_error(io::Error::last_os_error(), &self.key, &path));
        }
        let count = count as usize;
        if count == bytes.len() {
            return Err(unsafe_path(&self.key, &path, "the symbolic link target is too long"));
        }
        bytes.truncate(count);
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }

    /// The regular file `name` in this directory, not a link, this user's with no permission bit in `mask`, read to
    /// at most `limit` bytes.
    pub fn read(&self, name: &str, mask: u32, limit: u64) -> Result<Vec<u8>, Check> {
        let path = self.path.join(name);
        let fd =
            self.openat(name, libc::O_RDONLY | libc::O_NONBLOCK, 0).map_err(|e| open_error(e, &self.key, &path))?;
        check_fd(&fd, false, mask, &self.key, &path)?;
        let mut bytes = Vec::new();
        File::from(fd)
            .take(limit)
            .read_to_end(&mut bytes)
            .map_err(|e| unsafe_path(&self.key, &path, &e.to_string()))?;
        Ok(bytes)
    }

    /// Writes `name` atomically: a new 0600 file beside it, synced, renamed over it, the directory synced.
    pub fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let staged = format!(".{name}.{}", std::process::id());
        let c_staged = CString::new(staged.as_str()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let c_name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: NUL-terminated names relative to an open directory.
        unsafe { libc::unlinkat(self.fd.as_raw_fd(), c_staged.as_ptr(), 0) };
        let written = (|| {
            let fd = self.openat(&staged, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
            let mut file = File::from(fd);
            file.write_all(bytes)?;
            file.sync_all()?;
            // SAFETY: both names NUL-terminated, both relative to the same open directory.
            if unsafe { libc::renameat(self.fd.as_raw_fd(), c_staged.as_ptr(), self.fd.as_raw_fd(), c_name.as_ptr()) }
                != 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the directory's fd is open.
            unsafe { libc::fsync(self.fd.as_raw_fd()) };
            Ok(())
        })();
        if written.is_err() {
            // SAFETY: as above.
            unsafe { libc::unlinkat(self.fd.as_raw_fd(), c_staged.as_ptr(), 0) };
        }
        written
    }

    /// Removes `name`; nothing to do when it is not there.
    pub fn remove(&self, name: &str) -> io::Result<()> {
        let c_name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: a NUL-terminated name relative to an open directory.
        if unsafe { libc::unlinkat(self.fd.as_raw_fd(), c_name.as_ptr(), 0) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ENOENT) {
                return Err(e);
            }
        }
        Ok(())
    }
}

fn one_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("svtr").tempdir_in("/tmp").unwrap()
    }

    #[test]
    fn ancestry_others_can_write_is_refused_unless_sticky() {
        let tmp = tmp();
        let shared = tmp.path().join("shared");
        let data = shared.join("sidevoice");
        std::fs::create_dir_all(&data).unwrap();
        chmod(&shared, 0o755);
        assert!(ancestry(&data, "install.unsafe").is_ok());
        chmod(&shared, 0o777);
        let Err(Check::Unsafe(refusal)) = ancestry(&data, "install.unsafe") else { panic!("a writable parent passed") };
        assert_eq!(refusal.key, "install.unsafe");
        chmod(&shared, 0o775);
        assert!(ancestry(&data, "install.unsafe").is_err(), "group-writable");
        chmod(&shared, 0o1777);
        assert!(ancestry(&data, "install.unsafe").is_ok(), "sticky, as /tmp");
        assert_eq!(ancestry(&tmp.path().join("none"), "k"), Err(Check::Missing));
    }

    #[test]
    fn a_directory_replaced_after_it_was_opened_is_not_read() {
        let tmp = tmp();
        let data = tmp.path().join("d");
        std::fs::create_dir(&data).unwrap();
        chmod(&data, 0o700);
        std::fs::write(data.join("install.json"), b"genuine").unwrap();
        let dir = Dir::open(&data, 0o022, "install.unsafe").unwrap();
        // Between the check and the read, the directory is swapped for another with another file in it.
        std::fs::rename(&data, tmp.path().join("d.old")).unwrap();
        std::fs::create_dir(&data).unwrap();
        std::fs::write(data.join("install.json"), b"replacement").unwrap();
        assert_eq!(dir.read("install.json", 0o022, 1024).unwrap(), b"genuine");
    }

    #[test]
    fn links_and_loose_files_are_refused() {
        let tmp = tmp();
        let data = tmp.path().join("d");
        std::fs::create_dir(&data).unwrap();
        chmod(&data, 0o700);
        std::fs::write(tmp.path().join("elsewhere.json"), b"x").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("elsewhere.json"), data.join("install.json")).unwrap();
        let dir = Dir::open(&data, 0o022, "install.unsafe").unwrap();
        assert!(matches!(dir.read("install.json", 0o022, 1024), Err(Check::Unsafe(_))), "a link");
        assert_eq!(dir.read("none.json", 0o022, 1024), Err(Check::Missing));
        std::fs::write(data.join("loose.json"), b"x").unwrap();
        chmod(&data.join("loose.json"), 0o666);
        assert!(matches!(dir.read("loose.json", 0o022, 1024), Err(Check::Unsafe(_))));
        chmod(&data.join("loose.json"), 0o644);
        assert!(dir.read("loose.json", 0o022, 1024).is_ok());
        assert!(matches!(dir.read("loose.json", 0o077, 1024), Err(Check::Unsafe(_))), "a credential must be private");
        // The directory itself: a link to it, or a loose mode, is refused.
        std::os::unix::fs::symlink(&data, tmp.path().join("link")).unwrap();
        assert!(Dir::open(&tmp.path().join("link"), 0o022, "k").is_ok(), "a link above is resolved first");
        chmod(&data, 0o777);
        assert!(matches!(Dir::open(&data, 0o022, "k"), Err(Check::Unsafe(_))));
    }

    #[test]
    fn write_is_atomic_and_private() {
        let tmp = tmp();
        chmod(tmp.path(), 0o700);
        let dir = Dir::open(tmp.path(), 0o077, "app.storage").unwrap();
        dir.write("local-host.json", b"one").unwrap();
        dir.write("local-host.json", b"two").unwrap();
        assert_eq!(dir.read("local-host.json", 0o077, 1024).unwrap(), b"two");
        assert_eq!(std::fs::metadata(tmp.path().join("local-host.json")).unwrap().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
        dir.remove("local-host.json").unwrap();
        dir.remove("local-host.json").unwrap();
    }

    #[test]
    fn executables_need_a_safe_file_and_ancestry() {
        let tmp = tmp();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        chmod(&bin, 0o755);
        let program = bin.join("node");
        std::fs::write(&program, b"#!/bin/sh\n").unwrap();
        chmod(&program, 0o755);
        assert_eq!(executable(&program), Ok(std::fs::canonicalize(&program).unwrap()));
        chmod(&program, 0o777);
        assert!(executable(&program).is_err());
        chmod(&program, 0o755);
        chmod(&bin, 0o777);
        assert!(executable(&program).is_err());
        chmod(&bin, 0o755);
        assert_eq!(
            executable(Path::new("/bin/sh")),
            Ok(std::fs::canonicalize("/bin/sh").unwrap()),
            "a system link resolved"
        );
    }
}
