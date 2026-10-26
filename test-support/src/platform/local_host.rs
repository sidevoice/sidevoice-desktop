//! The local host's two CI-only modes, which need the Unix-only `sidevoice-local-host` crate and a second user.
use std::io;

#[cfg(unix)]
#[path = "local_host/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "local_host/windows.rs"]
mod imp;

pub use imp::Os as Ci;

/// What `local-host-ci` runs besides the stand-in core. A host without the local host (the crate is empty there until
/// local-host IPC is portable) answers [`io::ErrorKind::Unsupported`] and runs nothing: that is the availability answer.
pub trait LocalHostCi {
    /// As the first user: the stand-in core and the app's side paired with it, the proxy on loopback. Writes the
    /// proxy's port (only the port) to `port_file` and serves until killed. Leaves `data` open for a second user to
    /// meet the core directory's own refusal.
    fn serve(data: &str, app: &str, port_file: &str) -> io::Result<()>;

    /// As a second user: must fail to open the core's `socket`, to read the app's pairing and to use the proxy at
    /// `port` without the secret. Prints one line per attempt; the process exits 1 if any of them got through.
    fn attack(data: &str, app: &str, port: u16, socket: &str) -> io::Result<()>;
}
