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
    let connected = block_on(async { tokio::time::timeout(CONNECT_WAIT, Channels::connect(endpoint)).await });
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
        server.join().unwrap();
        assert_eq!(stream.read(&mut echoed).unwrap(), 0, "ended");
        assert_eq!(stream.write(b"x").unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn nobody_listening_is_not_found_or_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let error = connect(&endpoint(&tmp, "t")).unwrap_err();
        assert!(matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused), "{error:?}");
    }

    #[test]
    fn a_read_past_its_deadline_fails_and_the_connection_stays_usable() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = endpoint(&tmp, "d");
        let listener = listen(&endpoint).unwrap();
        let stream = connect(&endpoint).unwrap();
        let mut server = accept(&listener).unwrap();
        let clone = stream.try_clone().unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let mut byte = [0u8; 1];
        let started = Instant::now();
        assert_eq!((&clone).read(&mut byte).unwrap_err().kind(), io::ErrorKind::TimedOut, "clones share it");
        assert!(started.elapsed() < Duration::from_secs(5));
        server.write_all(b"x").unwrap();
        (&stream).read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"x");
        assert_eq!(stream.set_read_timeout(Some(Duration::ZERO)).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn shutdown_on_one_handle_stops_a_read_blocked_on_another() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = endpoint(&tmp, "s");
        let listener = listen(&endpoint).unwrap();
        let stream = connect(&endpoint).unwrap();
        let _server = accept(&listener).unwrap();
        let reader = stream.try_clone().unwrap();
        let blocked = std::thread::spawn(move || (&reader).read(&mut [0u8; 1]));
        std::thread::sleep(Duration::from_millis(100));
        stream.shutdown().unwrap();
        assert_eq!(blocked.join().unwrap().unwrap(), 0);
    }
}
