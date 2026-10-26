//! Child processes as the local host runs them, and whether a recorded process is still there.
//!
//! A program runs as a tree of its own ([`spawn_tree`]): `process-wrap`'s process group on Unix, so whatever it starts
//! can be killed with it, also after it has exited. Its output is read without blocking past a deadline
//! ([`Output::drain`]). What differs per OS is in `process/<os>.rs`: how a tree is contained, how to wait for a pipe
//! to have something, how to ask whether a process exists, and how large a process id can be. What counts as alive,
//! how often a busy program is retried and how much output is kept are decided here.

use process_wrap::std::{ChildWrapper, CommandWrap};
use std::io::{self, Read};
use std::process::{ChildStderr, ChildStdout, Command, ExitStatus};
use std::thread;
use std::time::Duration;

#[cfg_attr(unix, path = "process/unix.rs")]
#[cfg_attr(windows, path = "process/windows.rs")]
mod imp;
use imp::Native;

/// What differs per OS. A host that cannot do something answers [`io::ErrorKind::Unsupported`].
trait Os {
    /// The largest process id the OS hands out.
    const MAX_PID: u64;

    /// Makes `command` start a tree of its own, which [`Tree::kill_tree`] kills whole.
    fn contain(_command: &mut CommandWrap) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "running a program is not available on this OS yet"))
    }

    /// Waits at most `wait` for any of `pipes` to have something to read (or to be closed), and says which do, in
    /// order. With no pipes, only waits.
    fn ready(_pipes: &[&Pipe], _wait: Duration) -> io::Result<Vec<bool>> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    /// Whether a process with id `pid` exists, whoever it belongs to.
    fn exists(_pid: u32) -> io::Result<bool> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// A process id read from a record: one the OS could hand out, never 0.
pub fn pid(raw: u64) -> Option<u32> {
    (raw > 0 && raw <= Native::MAX_PID).then_some(raw as u32)
}

/// Whether process `pid` is there. One that exists but is not ours still is; one that cannot be asked about is not.
pub fn alive(pid: u32) -> bool {
    Native::exists(pid).unwrap_or(false)
}

/// Starts `command` as a tree of its own. A program being written at that instant (an update, or any process that
/// forked while holding it open for writing) cannot be run yet: it is tried again shortly, five times 50 ms apart.
pub fn spawn_tree(command: Command) -> io::Result<Tree> {
    let mut command = CommandWrap::from(command);
    Native::contain(&mut command)?;
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && tries < 5 => {
                tries += 1;
                thread::sleep(Duration::from_millis(50));
            }
            spawned => return spawned.map(|child| Tree { child }),
        }
    }
}

/// A program started by [`spawn_tree`], and everything it started that stayed in its tree.
pub struct Tree {
    child: Box<dyn ChildWrapper>,
}

impl Tree {
    /// The program's own exit, when it has exited; nothing else in the tree is waited for.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.inner_mut().try_wait()
    }

    /// Kills whatever is left in the tree; nobody left is fine.
    pub fn kill_tree(&mut self) {
        let _ = self.child.start_kill();
    }

    /// Kills the whole tree, the program too, and reaps the program.
    pub fn stop(&mut self) {
        self.kill_tree();
        let program = self.child.inner_mut();
        let _ = program.kill();
        let _ = program.wait();
    }

    /// The program's standard output and error, when they were piped; each keeps at most `limit` bytes.
    pub fn output(&mut self, limit: u64) -> Output {
        let stdout = self.child.stdout().take().map(Pipe::Out);
        let stderr = self.child.stderr().take().map(Pipe::Err);
        Output { streams: [Stream::new(stdout), Stream::new(stderr)], limit }
    }
}

/// One of a child's output pipes.
enum Pipe {
    Out(ChildStdout),
    Err(ChildStderr),
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Pipe::Out(pipe) => pipe.read(buf),
            Pipe::Err(pipe) => pipe.read(buf),
        }
    }
}

/// A pipe, read until it closes, and what came out of it.
struct Stream {
    pipe: Option<Pipe>,
    bytes: Vec<u8>,
}

impl Stream {
    fn new(pipe: Option<Pipe>) -> Self {
        Stream { pipe, bytes: Vec::new() }
    }
}

/// A child's standard output and error, read as they come ([`Tree::output`]).
pub struct Output {
    streams: [Stream; 2],
    limit: u64,
}

impl Output {
    /// Whether either stream is still open.
    pub fn is_open(&self) -> bool {
        self.streams.iter().any(|s| s.pipe.is_some())
    }

    /// Waits at most `wait` for any open stream to have something, and reads what there is. End of file, or an error,
    /// closes the stream; beyond the limit bytes are read and dropped.
    pub fn drain(&mut self, wait: Duration) {
        let pipes: Vec<&Pipe> = self.streams.iter().filter_map(|s| s.pipe.as_ref()).collect();
        let Ok(ready) = Native::ready(&pipes, wait) else { return };
        let mut ready = ready.into_iter();
        for stream in self.streams.iter_mut().filter(|s| s.pipe.is_some()) {
            if !ready.next().unwrap_or(false) {
                continue;
            }
            let mut chunk = [0u8; 16 * 1024];
            match stream.pipe.as_mut().expect("open").read(&mut chunk) {
                Ok(0) | Err(_) => stream.pipe = None,
                Ok(n) if (stream.bytes.len() as u64) < self.limit => stream.bytes.extend_from_slice(&chunk[..n]),
                Ok(_) => {}
            }
        }
    }

    /// What each stream wrote: standard output, then standard error.
    pub fn into_bytes(self) -> (Vec<u8>, Vec<u8>) {
        let [stdout, stderr] = self.streams.map(|s| s.bytes);
        (stdout, stderr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_an_impossible_pid_is_none() {
        assert!(alive(std::process::id()));
        assert_eq!(pid(0), None);
        assert_eq!(pid(Native::MAX_PID + 1), None);
        assert_eq!(pid(u64::from(std::process::id())), Some(std::process::id()));
    }

    #[test]
    fn an_exited_process_is_not_alive() {
        // This test binary, asked only to list its tests.
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let gone = child.id();
        child.wait().unwrap();
        assert!(!alive(gone));
    }
}
