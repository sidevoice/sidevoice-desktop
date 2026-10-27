//! The local channel to the core and the connector as the local host uses it: `sidevoice_platform::ipc`, driven from
//! blocking code. Where to connect is the record the other side publishes ([`Endpoint`], parsed back by
//! `sidevoice-platform`, never computed here); connecting, and refusing a server that is not this user, is
//! `Channels::connect`. Nothing here differs per OS.
//!
//! What is added for blocking callers is decided here, once: how long a connect may wait ([`CONNECT_WAIT`]), deadlines
//! on reads and writes, more than one handle on a connection, and ending it from another thread.

use crate::runtime::block_on;
use sidevoice_platform::ipc::{Channel, Channels, Endpoint, Stream as Channel_};
use std::future::Future;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

/// How long a connect may wait for the other side to take it.
pub const CONNECT_WAIT: Duration = Duration::from_secs(2);

/// One connection to a local endpoint, its server checked to be this user. Reads and writes block, bounded by
/// [`Stream::set_read_timeout`] and [`Stream::set_write_timeout`]; a read or write past its deadline fails as
/// [`io::ErrorKind::TimedOut`] having taken or sent nothing, and the connection stays usable.
#[derive(Clone)]
pub struct Stream(Arc<Shared>);

/// What every handle on a connection shares.
struct Shared {
    /// The connection, until [`Stream::shutdown`] drops it.
    channel: RwLock<Option<Channel_>>,
    /// Set once by [`Stream::shutdown`]: whatever is in progress on the connection stops.
    ended: watch::Sender<bool>,
    read_timeout: Mutex<Option<Duration>>,
    write_timeout: Mutex<Option<Duration>>,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stream")
    }
}

/// Connects to `endpoint`, refusing a server that is not this user ([`io::ErrorKind::PermissionDenied`]). Nobody
/// listening is [`io::ErrorKind::NotFound`] or [`io::ErrorKind::ConnectionRefused`]; nobody taking the connection
/// within [`CONNECT_WAIT`] is [`io::ErrorKind::TimedOut`].
pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    connect_within(endpoint, CONNECT_WAIT)
}

/// [`connect`], waiting at most `wait` for the other side to take the connection.
fn connect_within(endpoint: &Endpoint, wait: Duration) -> io::Result<Stream> {
    let connected = block_on(async { tokio::time::timeout(wait, Channels::connect(endpoint)).await });
    let channel = connected.map_err(|_| {
        io::Error::new(io::ErrorKind::TimedOut, format!("{endpoint} did not take the connection in time"))
    })??;
    Ok(Stream::new(channel))
}

impl Stream {
    fn new(channel: Channel_) -> Stream {
        Stream(Arc::new(Shared {
            channel: RwLock::new(Some(channel)),
            ended: watch::Sender::new(false),
            read_timeout: Mutex::new(None),
            write_timeout: Mutex::new(None),
        }))
    }

    /// The deadline of each read from now on, on every handle of this connection; `None` waits as long as it takes.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        *self.0.read_timeout.lock().unwrap_or_else(PoisonError::into_inner) = nonzero(timeout)?;
        Ok(())
    }

    /// The deadline of each write from now on, on every handle of this connection; `None` waits as long as it takes.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        *self.0.write_timeout.lock().unwrap_or_else(PoisonError::into_inner) = nonzero(timeout)?;
        Ok(())
    }

    /// Another handle on the same connection (to read on one thread and write on another, or to end it from outside).
    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(self.clone())
    }

    /// Ends the connection for every handle on it: whatever is in progress on another thread stops, later reads
    /// return 0 and writes fail ([`io::ErrorKind::BrokenPipe`]), and the other side sees it closed.
    pub fn shutdown(&self) -> io::Result<()> {
        self.0.ended.send_replace(true);
        // Every read or write in progress has seen `ended` and let go; dropping the connection closes it.
        self.0.channel.write().unwrap_or_else(PoisonError::into_inner).take();
        Ok(())
    }

    /// Does `op` on the connection within its deadline, unless the connection is or gets ended first. Bytes read or
    /// written, 0 for a flush.
    fn run(&self, op: Op<'_>) -> io::Result<usize> {
        let (timeout, on_end): (_, fn() -> io::Result<usize>) = match op {
            Op::Read(_) => (&self.0.read_timeout, ended_read),
            Op::Write(_) | Op::Flush => (&self.0.write_timeout, ended_write),
        };
        let channel = self.0.channel.read().unwrap_or_else(PoisonError::into_inner);
        let Some(mut channel) = channel.as_ref() else { return on_end() };
        let timeout = *timeout.lock().unwrap_or_else(PoisonError::into_inner);
        let mut ended = self.0.ended.subscribe();
        block_on(async {
            let io = async {
                match op {
                    Op::Read(buf) => channel.read(buf).await,
                    Op::Write(buf) => channel.write(buf).await,
                    Op::Flush => channel.flush().await.map(|()| 0),
                }
            };
            tokio::select! {
                biased;
                _ = ended.wait_for(|ended| *ended) => on_end(),
                done = within(timeout, io) => done,
            }
        })
    }
}

/// A timeout of zero is refused, as std's sockets refuse it.
fn nonzero(timeout: Option<Duration>) -> io::Result<Option<Duration>> {
    match timeout {
        Some(timeout) if timeout.is_zero() => Err(io::Error::new(io::ErrorKind::InvalidInput, "a zero timeout")),
        timeout => Ok(timeout),
    }
}

/// `io`, failing as [`io::ErrorKind::TimedOut`] past `timeout`. A read or write given up on has taken or sent nothing:
/// the channel's reads and writes are cancel-safe.
async fn within<T>(timeout: Option<Duration>, io: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    match timeout {
        Some(timeout) => {
            tokio::time::timeout(timeout, io).await.unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
        }
        None => io.await,
    }
}

fn ended_read() -> io::Result<usize> {
    Ok(0)
}

/// A read, a write or a flush ([`Stream::run`]).
enum Op<'b> {
    Read(&'b mut [u8]),
    Write(&'b [u8]),
    Flush,
}

fn ended_write() -> io::Result<usize> {
    Err(io::Error::new(io::ErrorKind::BrokenPipe, "the connection was ended"))
}

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.run(Op::Read(buf))
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.run(Op::Write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.run(Op::Flush).map(|_| ())
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

/// A listener at `endpoint` (`Channels::listen`), for tests that need somebody answering.
#[cfg(test)]
pub(crate) fn listen(endpoint: &Endpoint) -> io::Result<sidevoice_platform::ipc::Listener> {
    block_on(async { Channels::listen(endpoint) })
}

/// The next connection on `listener`, as a blocking [`Stream`].
#[cfg(test)]
pub(crate) fn accept(listener: &sidevoice_platform::ipc::Listener) -> io::Result<Stream> {
    block_on(listener.accept()).map(Stream::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_test_support::platform::storage::{StorageFixture, StorageFixtures};
    use sidevoice_platform::ipc::Address as _;
    use std::time::Instant;

    /// How long a test waits for a thread that should stop: past it, the test fails instead of hanging.
    const STOP_LIMIT: Duration = Duration::from_secs(20);

    /// What `thread` returned; a failure naming `what` when it is still running after [`STOP_LIMIT`].
    fn joined<T>(thread: std::thread::JoinHandle<T>, what: &str) -> T {
        let started = Instant::now();
        while !thread.is_finished() {
            assert!(started.elapsed() < STOP_LIMIT, "{what} did not stop within {STOP_LIMIT:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
        thread.join().unwrap()
    }

    /// The endpoint `name` for a scratch directory open to this user alone, where a listener may be made.
    fn endpoint(dir: &tempfile::TempDir, name: &str) -> Endpoint {
        StorageFixtures::private(dir.path());
        Endpoint::new(dir.path(), name)
    }

    #[test]
    fn this_users_own_listener_answers_and_bytes_flow_both_ways() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = endpoint(&tmp, "t");
        let listener = listen(&endpoint).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = accept(&listener).unwrap();
            let mut line = [0u8; 5];
            stream.read_exact(&mut line).unwrap();
            stream.write_all(&line).unwrap();
            // Until the client ends the connection.
            assert_eq!(stream.read(&mut line).unwrap(), 0);
        });
        let mut stream = connect(&endpoint).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(b"hello").unwrap();
        let mut echoed = [0u8; 5];
        stream.try_clone().unwrap().read_exact(&mut echoed).unwrap();
        assert_eq!(&echoed, b"hello");
        stream.shutdown().unwrap();
        joined(server, "the echo server, once the connection ended");
        assert_eq!(stream.read(&mut echoed).unwrap(), 0, "ended");
        assert_eq!(stream.write(b"x").unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn nobody_listening_is_not_found_or_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let error = connect(&endpoint(&tmp, "t")).unwrap_err();
        assert!(matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused), "{error:?}");
    }

    /// A connection to a listener that accepts and then says nothing, kept open until the returned sender is dropped.
    fn silent() -> (tempfile::TempDir, Stream, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = endpoint(&tmp, "t");
        let listener = listen(&endpoint).unwrap();
        let (hold, release) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let _stream = accept(&listener).unwrap();
            let _ = release.recv();
        });
        let stream = connect(&endpoint).unwrap();
        (tmp, stream, hold, server)
    }

    #[test]
    fn a_read_past_its_deadline_fails_and_the_connection_stays_usable() {
        let (_tmp, stream, hold, server) = silent();
        stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let started = Instant::now();
        let reader = stream.try_clone().unwrap();
        let reading = std::thread::spawn(move || (&reader).read(&mut [0u8; 8]).unwrap_err());
        assert_eq!(joined(reading, "a read past its deadline").kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5), "bounded: {:?}", started.elapsed());
        // A clone shares the deadline.
        let mut clone = stream.try_clone().unwrap();
        let error = joined(std::thread::spawn(move || clone.read(&mut [0u8; 8]).unwrap_err()), "the clone's read");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        (&stream).write_all(b"still open").unwrap();
        assert_eq!(stream.set_read_timeout(Some(Duration::ZERO)).unwrap_err().kind(), io::ErrorKind::InvalidInput);
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
        let endpoint = endpoint(&tmp, "t");
        // Never accepts: a pipe's one instance is taken by the first client, a socket's backlog keeps the rest.
        let _listener = listen(&endpoint).unwrap();
        let wait = Duration::from_millis(300);
        let mut held = Vec::new();
        for _ in 0..3 {
            let started = Instant::now();
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
