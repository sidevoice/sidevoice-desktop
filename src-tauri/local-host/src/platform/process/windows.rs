//! Windows: a Job Object for the tree and no console window of its own (`process-wrap`, as `sidevoice_platform`'s
//! `spawn_tree` does); `PeekNamedPipe` to wait on the child's pipes (std's child pipes are pipes, and nothing
//! maintained waits on them: own `unsafe`, one call); `OpenProcess` and a wait for no time to ask whether a process is
//! running, as the app's webview release asks (own `unsafe`, three calls).

use super::{Os, Pipe};
use process_wrap::std::{CommandWrap, CreationFlags, JobObject};
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Pipes::PeekNamedPipe;
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_SYNCHRONIZE};

/// How often the pipes are looked at again while nothing has come.
const LOOK_EVERY: Duration = Duration::from_millis(10);

pub struct Native;

impl Os for Native {
    /// A `DWORD`.
    const MAX_PID: u64 = u32::MAX as u64;

    /// Whatever the program starts is in its Job Object, detached or not: `process-wrap`'s job does not allow breaking
    /// away (`DETACHED_LEAVES_THE_TREE` keeps its `false`); killing the job kills them all, also after the program has
    /// exited.
    fn contain(command: &mut CommandWrap) -> io::Result<()> {
        command.wrap(CreationFlags(CREATE_NO_WINDOW)).wrap(JobObject);
        Ok(())
    }

    fn ready(pipes: &[&Pipe], wait: Duration) -> io::Result<Vec<bool>> {
        let until = Instant::now() + wait.clamp(Duration::from_millis(1), Duration::from_secs(1));
        loop {
            let ready: Vec<bool> = pipes.iter().map(|pipe| has_input(handle(pipe))).collect();
            let left = until.saturating_duration_since(Instant::now());
            if ready.contains(&true) || left.is_zero() {
                return Ok(ready);
            }
            thread::sleep(left.min(LOOK_EVERY));
        }
    }

    /// Opened for waiting only and waited on for no time: running while the wait times out. No such process is an
    /// invalid parameter; one this user may not open (another user's, a protected one) is there, as signal 0's `EPERM`
    /// is on Unix. Any other failure says nothing about `pid`.
    fn exists(pid: u32) -> io::Result<bool> {
        // SAFETY: a synchronization-only handle is asked for; nothing of the process is read or changed.
        let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            Ok(process) => process,
            Err(e) => {
                let e = io::Error::from(e);
                return match e.raw_os_error() {
                    Some(code) if code == ERROR_INVALID_PARAMETER.0 as i32 => Ok(false),
                    Some(code) if code == ERROR_ACCESS_DENIED.0 as i32 => Ok(true),
                    _ => Err(e),
                };
            }
        };
        // SAFETY: the handle was just opened; it is waited on without blocking and closed once, on every path.
        let waited = unsafe { WaitForSingleObject(process, 0) };
        let error = io::Error::last_os_error();
        // SAFETY: as above.
        let _ = unsafe { CloseHandle(process) };
        match waited {
            WAIT_TIMEOUT => Ok(true),
            WAIT_OBJECT_0 => Ok(false),
            _ => Err(error),
        }
    }
}

/// Whether `pipe` has something to read, or is closed or failing (the read that follows then ends it).
fn has_input(pipe: BorrowedHandle<'_>) -> bool {
    let mut available = 0u32;
    // SAFETY: the handle is open for the call's duration; only the count of available bytes is written.
    let peeked = unsafe { PeekNamedPipe(HANDLE(pipe.as_raw_handle()), None, 0, None, Some(&mut available), None) };
    peeked.is_err() || available > 0
}

fn handle<'a>(pipe: &'a Pipe) -> BorrowedHandle<'a> {
    match pipe {
        Pipe::Out(pipe) => pipe.as_handle(),
        Pipe::Err(pipe) => pipe.as_handle(),
    }
}
