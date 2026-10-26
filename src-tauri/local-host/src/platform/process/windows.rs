//! Windows: a Job Object for the tree and no console window of its own (`process-wrap`, as `sidevoice_platform`'s
//! `spawn_tree` does); `PeekNamedPipe` to wait on the child's pipes (std's child pipes are pipes, and nothing
//! maintained waits on them: own `unsafe`, one call); `sysinfo` to ask whether a process exists.

use super::{Os, Pipe};
use process_wrap::std::{CommandWrap, CreationFlags, JobObject};
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle};
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Pipes::PeekNamedPipe;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

/// How often the pipes are looked at again while nothing has come.
const LOOK_EVERY: Duration = Duration::from_millis(10);

pub struct Native;

impl Os for Native {
    /// A `DWORD`.
    const MAX_PID: u64 = u32::MAX as u64;

    /// Whatever the program starts is in its Job Object unless it breaks away; killing the job kills them all, also
    /// after the program has exited.
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

    /// In the system's process list, whoever it belongs to. This process is always in it: when it is not, the list
    /// could not be read, and nothing is said about `pid`.
    fn exists(pid: u32) -> io::Result<bool> {
        let me = sysinfo::get_current_pid().map_err(io::Error::other)?;
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid, me]), true, ProcessRefreshKind::nothing());
        if system.process(me).is_none() {
            return Err(io::Error::other("the system's process list cannot be read"));
        }
        Ok(system.process(pid).is_some())
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
