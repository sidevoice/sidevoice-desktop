//! Windows: a named pipe, through `interprocess`.
//!
//! Who is at the other end: the pipe's server process (`GetNamedPipeServerProcessId`, through `interprocess`), named
//! by the user SID in its token, read through `sysinfo` like this process's own: the check `sidevoice_platform::ipc`
//! makes on its own pipes. A process whose token cannot be read is not a user anybody is compared with: an error, so
//! the peer is not trusted.
//!
//! `interprocess`'s named pipes have no timeouts, and a pipe has no half-close; what a connection's clones share keeps
//! both. A read or write with a deadline runs while a watchdog thread waits for it; past the deadline the watchdog
//! cancels the pipe's I/O (`CancelIoEx`: `interprocess` does its I/O overlapped, so another thread can cancel it) until
//! the operation returns, which then fails as `TimedOut`. Ending a connection marks it ended and cancels whatever is
//! under way on it, the same way, until nothing is; from then on its reads give 0 and its writes fail. The server sees
//! it closed once every handle on it is dropped.

use super::Os;
use interprocess::local_socket::traits::StreamCommon as _;
use interprocess::local_socket::Stream as Local;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::CancelIoEx;

/// How often an operation past its deadline, or on an ended connection, is cancelled again until it returns: it may
/// not have reached the pipe yet when it is first cancelled.
const CANCEL_AGAIN: Duration = Duration::from_millis(5);
/// How long ending a connection waits for what is under way on it to stop.
const END_WAIT: Duration = Duration::from_secs(2);

/// A user as the SID in a process's token.
#[derive(Debug, PartialEq)]
pub struct Sid(sysinfo::Uid);

impl fmt::Display for Sid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "user {}", *self.0)
    }
}

/// What a pipe connection's clones share: its deadlines, whether it was ended, and how many reads and writes are under
/// way on it.
#[derive(Debug, Default)]
pub struct Shared {
    read: Mutex<Option<Duration>>,
    write: Mutex<Option<Duration>>,
    ended: AtomicBool,
    busy: AtomicUsize,
}

pub struct Native;

impl Os for Native {
    type User = Sid;
    type Shared = Shared;

    fn this_user() -> io::Result<Sid> {
        user_of(sysinfo::get_current_pid().map_err(io::Error::other)?)
    }

    fn peer_user(stream: &Local) -> io::Result<Sid> {
        let pid = stream.peer_creds()?.pid().ok_or_else(|| io::Error::other("the pipe does not say who serves it"))?;
        user_of(Pid::from_u32(pid))
    }

    fn set_read_timeout(_stream: &Local, shared: &Shared, timeout: Option<Duration>) -> io::Result<()> {
        set(&shared.read, timeout)
    }

    fn set_write_timeout(_stream: &Local, shared: &Shared, timeout: Option<Duration>) -> io::Result<()> {
        set(&shared.write, timeout)
    }

    fn read(stream: &Local, shared: &Shared, buf: &mut [u8]) -> io::Result<usize> {
        let mut reader = stream;
        Ok(bounded(stream, shared, get(&shared.read), || reader.read(buf))?.unwrap_or(0))
    }

    fn write(stream: &Local, shared: &Shared, buf: &[u8]) -> io::Result<usize> {
        let mut writer = stream;
        bounded(stream, shared, get(&shared.write), || writer.write(buf))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "the connection was ended"))
    }

    fn shutdown(stream: &Local, shared: &Shared) -> io::Result<()> {
        shared.ended.store(true, SeqCst);
        let until = Instant::now() + END_WAIT;
        while shared.busy.load(SeqCst) > 0 {
            if Instant::now() >= until {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "a read or write on the pipe did not stop"));
            }
            cancel(handle(stream));
            thread::sleep(CANCEL_AGAIN);
        }
        Ok(())
    }
}

/// The user SID of the running process `pid`; an error when it is not running or its token cannot be read.
fn user_of(pid: Pid) -> io::Result<Sid> {
    let mut system = System::new();
    let user = ProcessRefreshKind::nothing().with_user(UpdateKind::Always);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, user);
    let process = system
        .process(pid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("process {pid} is not running")))?;
    let user = process.user_id().cloned();
    user.map(Sid).ok_or_else(|| {
        io::Error::new(io::ErrorKind::PermissionDenied, format!("the user of process {pid} cannot be read"))
    })
}

fn set(deadline: &Mutex<Option<Duration>>, timeout: Option<Duration>) -> io::Result<()> {
    if timeout == Some(Duration::ZERO) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a zero timeout"));
    }
    *deadline.lock().unwrap_or_else(PoisonError::into_inner) = timeout;
    Ok(())
}

fn get(deadline: &Mutex<Option<Duration>>) -> Option<Duration> {
    *deadline.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `io` on `stream` within `timeout` (past it, `TimedOut`), counted as under way; `None` when the connection was
/// ended, before or while it ran.
fn bounded<T>(
    stream: &Local,
    shared: &Shared,
    timeout: Option<Duration>,
    io: impl FnOnce() -> io::Result<T>,
) -> io::Result<Option<T>> {
    // Counted before `ended` is read, and `shutdown` sets `ended` before it reads the count: an operation either sees
    // the connection ended or is cancelled by `shutdown` until it returns.
    shared.busy.fetch_add(1, SeqCst);
    let _leave = Leave(&shared.busy);
    if shared.ended.load(SeqCst) {
        return Ok(None);
    }
    let result = match timeout {
        Some(timeout) => watched(handle(stream), timeout, io),
        None => io(),
    };
    if shared.ended.load(SeqCst) {
        return Ok(None);
    }
    result.map(Some)
}

struct Leave<'a>(&'a AtomicUsize);

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, SeqCst);
    }
}

/// Runs `io` while a watchdog waits `timeout` for it; past that, the watchdog cancels `pipe`'s I/O until `io` returns,
/// and its failure is `TimedOut`. What `io` got in time is kept.
fn watched<T>(pipe: BorrowedHandle<'_>, timeout: Duration, io: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let (done, finished) = mpsc::channel::<()>();
    let late = AtomicBool::new(false);
    let result = thread::scope(|scope| {
        let late = &late;
        scope.spawn(move || {
            if finished.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
                late.store(true, SeqCst);
                cancel(pipe);
                while finished.recv_timeout(CANCEL_AGAIN) == Err(RecvTimeoutError::Timeout) {
                    cancel(pipe);
                }
            }
        });
        let result = io();
        drop(done);
        result
    });
    match result {
        Err(_) if late.load(SeqCst) => Err(io::Error::new(io::ErrorKind::TimedOut, "the pipe did not answer in time")),
        result => result,
    }
}

/// Cancels every read and write under way on `pipe`'s connection, from any thread of this process (every handle on
/// it shares one file object). Nothing under way is fine.
fn cancel(pipe: BorrowedHandle<'_>) {
    // SAFETY: the handle is open for the call's duration; no `OVERLAPPED` of ours is named, so nothing of ours is
    // released by the cancellation.
    let _ = unsafe { CancelIoEx(HANDLE(pipe.as_raw_handle()), None) };
}

fn handle(stream: &Local) -> BorrowedHandle<'_> {
    match stream {
        Local::NamedPipe(pipe) => pipe.inner().as_handle(),
    }
}
