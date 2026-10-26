//! The local channel to the core and the connector as the app uses it: blocking, one connection per request, with
//! deadlines on reads and writes. Where to connect is the record the other side publishes ([`Endpoint`], parsed back
//! by `sidevoice-platform`, never computed here); connecting goes through `interprocess`, the crate under
//! `sidevoice_platform::ipc`.
//!
//! What differs per OS is observed in `ipc/<os>.rs`: who is at the other end of a connection, who this process is,
//! and how a connection is ended from another thread. Whether the peer is this user is decided here, once.

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericFilePath, Stream as Local, ToFsName};
use interprocess::TryClone as _;
use sidevoice_platform::ipc::Endpoint;
use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

#[cfg_attr(unix, path = "ipc/unix.rs")]
#[cfg_attr(windows, path = "ipc/windows.rs")]
mod imp;
use imp::Native;

/// What differs per OS. A host that cannot tell answers [`io::ErrorKind::Unsupported`], and its peers are never
/// trusted.
trait Os {
    /// A user as this OS names one: compared with this process's, shown in a refusal.
    type User: PartialEq + fmt::Display;

    /// The user this process runs as: the trust boundary.
    fn this_user() -> io::Result<Self::User> {
        Err(unsupported())
    }

    /// The user of the process at the other end of `stream`, read from the connection itself.
    fn peer_user(_stream: &Local) -> io::Result<Self::User> {
        Err(unsupported())
    }

    /// Ends `stream` both ways, for every clone of it too, so whoever reads or writes it on another thread stops.
    fn shutdown(_stream: &Local) -> io::Result<()> {
        Err(unsupported())
    }

    /// An endpoint for a test's own listener in the scratch directory `dir`, nothing else using it.
    #[cfg(test)]
    fn scratch_endpoint(dir: &std::path::Path, name: &str) -> Endpoint {
        <Endpoint as sidevoice_platform::ipc::Address>::new(dir, name)
    }
}

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "the local channel is not available on this OS yet")
}

/// Who answers on a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Peer {
    /// This user: the app talks to it.
    ThisUser,
    /// Somebody else, as the OS names them (Unix: `uid 501`).
    Other(String),
}

/// One connection to a local endpoint. Reads and writes block, bounded by [`Stream::set_read_timeout`] and
/// [`Stream::set_write_timeout`].
#[derive(Debug)]
pub struct Stream(Local);

/// Connects to `endpoint`. Nothing about the peer is checked yet: that is [`Stream::peer`]. Nobody listening is
/// [`io::ErrorKind::NotFound`] or [`io::ErrorKind::ConnectionRefused`].
pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    let name = endpoint.to_string().to_fs_name::<GenericFilePath>()?;
    Local::connect(name).map(Stream)
}

impl Stream {
    /// Whether the process at the other end runs as this user.
    pub fn peer(&self) -> io::Result<Peer> {
        let peer = Native::peer_user(&self.0)?;
        Ok(if peer == Native::this_user()? { Peer::ThisUser } else { Peer::Other(peer.to_string()) })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_recv_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_send_timeout(timeout)
    }

    /// Another handle on the same connection (to read on one thread and write on another, or to end it from outside).
    pub fn try_clone(&self) -> io::Result<Stream> {
        self.0.try_clone().map(Stream)
    }

    /// Ends the connection both ways, for every clone of it too.
    pub fn shutdown(&self) -> io::Result<()> {
        Native::shutdown(&self.0)
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.0).read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.0).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.0).flush()
    }
}

/// An endpoint for a test's own listener in the scratch directory `dir`.
#[cfg(test)]
pub(crate) fn scratch_endpoint(dir: &std::path::Path, name: &str) -> Endpoint {
    Native::scratch_endpoint(dir, name)
}

/// A listener at `endpoint`, for tests that need somebody answering: the same crate, nothing checked.
#[cfg(test)]
pub(crate) fn listen(endpoint: &Endpoint) -> io::Result<interprocess::local_socket::Listener> {
    let name = endpoint.to_string().to_fs_name::<GenericFilePath>()?;
    interprocess::local_socket::ListenerOptions::new().name(name).create_sync()
}

#[cfg(test)]
mod tests {
    use super::*;
    use interprocess::local_socket::traits::Listener as _;

    #[test]
    fn this_users_own_listener_is_this_user_and_bytes_flow_both_ways() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = scratch_endpoint(tmp.path(), "t");
        let listener = listen(&endpoint).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let mut line = [0u8; 5];
            stream.read_exact(&mut line).unwrap();
            stream.write_all(&line).unwrap();
        });
        let mut stream = connect(&endpoint).unwrap();
        assert_eq!(stream.peer().unwrap(), Peer::ThisUser);
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(b"hello").unwrap();
        let mut echoed = [0u8; 5];
        stream.try_clone().unwrap().read_exact(&mut echoed).unwrap();
        assert_eq!(&echoed, b"hello");
        server.join().unwrap();
        stream.shutdown().unwrap();
        assert_eq!(stream.read(&mut echoed).unwrap(), 0, "ended");
    }

    #[test]
    fn nobody_listening_is_not_found_or_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let error = connect(&scratch_endpoint(tmp.path(), "t")).unwrap_err();
        assert!(matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused), "{error:?}");
    }
}
