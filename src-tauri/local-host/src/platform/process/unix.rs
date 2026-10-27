//! Unix: a process group (`process-wrap`), `poll` on the pipes and signal 0 to ask about a process (`rustix`), none of
//! it `unsafe` of ours.

use super::{Os, Pipe};
use process_wrap::std::{CommandWrap, ProcessGroup};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;
use rustix::process::{test_kill_process, Pid};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::time::Duration;

pub struct Native;

impl Os for Native {
    /// `pid_t` is signed.
    const MAX_PID: u64 = i32::MAX as u64;

    /// A session of its own is another process group.
    const DETACHED_LEAVES_THE_TREE: bool = true;

    fn contain(command: &mut CommandWrap) -> io::Result<()> {
        command.wrap(ProcessGroup::leader());
        Ok(())
    }

    fn ready(pipes: &[&Pipe], wait: Duration) -> io::Result<Vec<bool>> {
        let mut polled: Vec<PollFd> =
            pipes.iter().map(|pipe| PollFd::from_borrowed_fd(fd(pipe), PollFlags::IN)).collect();
        let millis = wait.as_millis().clamp(1, 1000) as i64;
        let timeout = Timespec { tv_sec: millis / 1000, tv_nsec: (millis % 1000) * 1_000_000 };
        if poll(&mut polled, Some(&timeout))? == 0 {
            return Ok(vec![false; pipes.len()]);
        }
        Ok(polled.iter().map(|p| !p.revents().is_empty()).collect())
    }

    /// Signal 0: nothing is sent. Not permitted still means it is there.
    fn exists(pid: u32) -> io::Result<bool> {
        let pid = i32::try_from(pid).ok().and_then(Pid::from_raw).ok_or(io::ErrorKind::InvalidInput)?;
        match test_kill_process(pid) {
            Ok(()) | Err(Errno::PERM) => Ok(true),
            Err(Errno::SRCH) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn fd(pipe: &Pipe) -> BorrowedFd<'_> {
    match pipe {
        Pipe::Out(pipe) => pipe.as_fd(),
        Pipe::Err(pipe) => pipe.as_fd(),
    }
}
