//! The local channel to the core and the connector as the app uses it: blocking, one connection per request, with
//! deadlines on reads and writes. Where to connect is the record the other side publishes ([`Endpoint`], parsed back
//! by `sidevoice-platform`, never computed here); connecting goes through `interprocess`, the crate under
//! `sidevoice_platform::ipc`.
//!
//! What differs per OS is observed in `ipc/<os>.rs`: who is at the other end of a connection, who this process is, how
//! reads and writes are bounded, and how a connection is ended from another thread. Whether the peer is this user is
//! decided here, once.

use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::{GenericFilePath, Stream as Local, ToFsName};
use interprocess::TryClone as _;
use sidevoice_platform::ipc::Endpoint;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;
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

    /// Connects to the endpoint recorded as `endpoint`, waiting at most `wait` for a free instance where a listener
    /// can have none (a busy pipe). As `interprocess` does: a socket's connect never waits for one.
    fn connect(endpoint: &str, _wait: Duration) -> io::Result<Local> {
        Local::connect(endpoint.to_fs_name::<GenericFilePath>()?)
    }

    /// The user this process runs as: the trust boundary.
    fn this_user() -> io::Result<Self::User> {
        Err(unsupported())
    }

    /// The user of the process at the other end of `stream`, read from the connection itself.
    fn peer_user(_stream: &Local) -> io::Result<Self::User> {
        Err(unsupported())
    }

    /// What a connection keeps beside `interprocess`'s stream, shared by all its clones: where the OS does not keep a
    /// connection's deadlines and its end itself (Windows), they are kept here.
    type Shared: Default + fmt::Debug + Send + Sync;

    /// Bounds every later read on `stream` and its clones by `timeout` (`None`: unbounded), as `interprocess` does.
    fn set_read_timeout(stream: &Local, _shared: &Self::Shared, timeout: Option<Duration>) -> io::Result<()> {
        stream.set_recv_timeout(timeout)
    }

    /// Bounds every later write on `stream` and its clones by `timeout` (`None`: unbounded), as `interprocess` does.
    fn set_write_timeout(stream: &Local, _shared: &Self::Shared, timeout: Option<Duration>) -> io::Result<()> {
        stream.set_send_timeout(timeout)
    }

    /// Reads from `stream` within its read timeout (past it, an error of kind `WouldBlock` or `TimedOut`); 0 once the
    /// connection ended, also by [`Os::shutdown`]. As `interprocess` does.
    fn read(stream: &Local, _shared: &Self::Shared, buf: &mut [u8]) -> io::Result<usize> {
        let mut stream = stream;
        stream.read(buf)
    }

    /// Writes to `stream` within its write timeout; an error once the connection ended. As `interprocess` does.
    fn write(stream: &Local, _shared: &Self::Shared, buf: &[u8]) -> io::Result<usize> {
        let mut stream = stream;
        stream.write(buf)
    }

    /// Flushes what this process still holds of `stream`'s writes. As `interprocess` does: nothing is held for a socket.
    fn flush(stream: &Local) -> io::Result<()> {
        let mut stream = stream;
        stream.flush()
    }

    /// Ends `stream` both ways, for every clone of it too, so whoever reads or writes it on another thread stops.
    fn shutdown(_stream: &Local, _shared: &Self::Shared) -> io::Result<()> {
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
pub struct Stream {
    local: Local,
    shared: Arc<<Native as Os>::Shared>,
}

/// How long connecting waits for a listener that has no free instance (Windows: every instance of the pipe taken,
/// by anyone who could create or open it) before giving up: the peer is only checked once connected.
const CONNECT_WAIT: Duration = Duration::from_secs(2);

/// Connects to `endpoint`. Nothing about the peer is checked yet: that is [`Stream::peer`]. Nobody listening is
/// [`io::ErrorKind::NotFound`] or [`io::ErrorKind::ConnectionRefused`]; a listener with no free instance for
/// [`CONNECT_WAIT`], an error too.
pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    connect_within(endpoint, CONNECT_WAIT)
}

fn connect_within(endpoint: &Endpoint, wait: Duration) -> io::Result<Stream> {
    Native::connect(&endpoint.to_string(), wait).map(|local| Stream { local, shared: Arc::default() })
}

impl Stream {
    /// Whether the process at the other end runs as this user.
    pub fn peer(&self) -> io::Result<Peer> {
        let peer = Native::peer_user(&self.local)?;
        Ok(if peer == Native::this_user()? { Peer::ThisUser } else { Peer::Other(peer.to_string()) })
    }

    /// Bounds every later read on this connection and its clones (`None`: unbounded).
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Native::set_read_timeout(&self.local, &self.shared, timeout)
    }

    /// Bounds every later write on this connection and its clones (`None`: unbounded).
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        Native::set_write_timeout(&self.local, &self.shared, timeout)
    }

    /// Another handle on the same connection (to read on one thread and write on another, or to end it from outside).
    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(Stream { local: self.local.try_clone()?, shared: Arc::clone(&self.shared) })
    }

    /// Ends the connection both ways, for every clone of it too.
    pub fn shutdown(&self) -> io::Result<()> {
        Native::shutdown(&self.local, &self.shared)
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
}

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Native::read(&self.local, &self.shared, buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Native::write(&self.local, &self.shared, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Native::flush(&self.local)
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

    /// How long a test waits for a thread that should stop: past it, the test fails instead of hanging.
    const STOP_LIMIT: Duration = Duration::from_secs(20);

    /// What `thread` returned; a failure naming `what` when it is still running after [`STOP_LIMIT`].
    fn joined<T>(thread: std::thread::JoinHandle<T>, what: &str) -> T {
        let started = std::time::Instant::now();
        while !thread.is_finished() {
            assert!(started.elapsed() < STOP_LIMIT, "{what} did not stop within {STOP_LIMIT:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        thread.join().unwrap()
    }

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
        joined(server, "the echo server");
        stream.shutdown().unwrap();
        assert_eq!(stream.read(&mut echoed).unwrap(), 0, "ended");
    }

    #[test]
    fn nobody_listening_is_not_found_or_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let error = connect(&scratch_endpoint(tmp.path(), "t")).unwrap_err();
        assert!(matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused), "{error:?}");
    }

    /// A connection to a listener that accepts and then says nothing, kept open until the returned sender is dropped.
    fn silent() -> (tempfile::TempDir, Stream, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = scratch_endpoint(tmp.path(), "t");
        let listener = listen(&endpoint).unwrap();
        let (hold, release) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let _stream = listener.accept().unwrap();
            let _ = release.recv();
        });
        let stream = connect(&endpoint).unwrap();
        (tmp, stream, hold, server)
    }

    #[test]
    fn a_read_past_its_deadline_fails_and_the_connection_stays_usable() {
        let (_tmp, stream, hold, server) = silent();
        stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let started = std::time::Instant::now();
        let reader = stream.try_clone().unwrap();
        let reading = std::thread::spawn(move || (&reader).read(&mut [0u8; 8]).unwrap_err());
        let error = joined(reading, "a read past its deadline");
        assert!(matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "bounded: {:?}", started.elapsed());
        // A clone shares the deadline.
        let mut clone = stream.try_clone().unwrap();
        let error = joined(std::thread::spawn(move || clone.read(&mut [0u8; 8]).unwrap_err()), "the clone's read");
        assert!(matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut), "{error:?}");
        (&stream).write_all(b"still open").unwrap();
        drop(hold);
        joined(server, "the silent server");
    }

    #[test]
    fn ending_a_connection_stops_a_read_blocked_on_another_thread() {
        let (_tmp, stream, hold, server) = silent();
        let mut reader = stream.try_clone().unwrap();
        let blocked = std::thread::spawn(move || reader.read(&mut [0u8; 8]));
        std::thread::sleep(Duration::from_millis(200));
        stream.shutdown().unwrap();
        assert_eq!(joined(blocked, "the blocked read").unwrap(), 0, "ended");
        assert!((&stream).write_all(b"x").is_err(), "nothing is written after the end");
        drop(hold);
        joined(server, "the silent server");
    }

    #[test]
    fn a_listener_with_no_free_instance_is_given_up_on_within_the_wait() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = scratch_endpoint(tmp.path(), "t");
        // Never accepts: a pipe's one instance is taken by the first client, a socket's backlog keeps the rest.
        let _listener = listen(&endpoint).unwrap();
        let wait = Duration::from_millis(300);
        let mut held = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            if let Ok(stream) = connect_within(&endpoint, wait) {
                held.push(stream);
            }
            assert!(started.elapsed() < wait + Duration::from_secs(3), "bounded: {:?}", started.elapsed());
        }
    }

    #[test]
    fn ending_a_connection_stops_a_blocked_write_and_a_read_on_other_threads() {
        let (_tmp, stream, hold, server) = silent();
        let mut reader = stream.try_clone().unwrap();
        let reading = std::thread::spawn(move || reader.read(&mut [0u8; 8]));
        let mut writer = stream.try_clone().unwrap();
        // Nobody reads the other end: the writes fill the buffers and then block.
        let writing = std::thread::spawn(move || {
            let chunk = vec![0u8; 64 * 1024];
            loop {
                if let Err(e) = writer.write(&chunk) {
                    return e;
                }
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        stream.shutdown().unwrap();
        assert_eq!(joined(reading, "the blocked read").unwrap(), 0, "the read ended");
        joined(writing, "the blocked write");
        drop(hold);
        joined(server, "the silent server");
    }
}
