//! Starting a fixture service as an independent owner's process.
use process_wrap::std::{ChildWrapper, CommandWrap};
use std::{io, process::Command};

#[cfg(unix)]
#[path = "process/unix.rs"]
mod imp;
#[cfg(windows)]
#[path = "process/windows.rs"]
mod imp;

use imp::Os;

/// The only part of starting a detached service that differs per OS.
trait Isolation {
    /// Whether what [`detach`] starts leaves the tree of whoever runs it (a session of its own), and so outlives that
    /// tree being killed. Not where the tree holds it (a Job Object allowing no breakaway).
    const LEAVES_AN_ENCLOSING_TREE: bool = false;

    /// Adds the `process-wrap` wrapper that detaches the child. None by default.
    fn isolate(_command: &mut CommandWrap) {}
}

/// Spawns `command` detached, as a service started on its own: a session of its own (Unix); on Windows a process
/// group of its own with no console, as the connector detaches the core, which stays in an enclosing job.
/// Whether what [`detach`] starts outlives the enclosing run's tree being killed (`sidevoice_platform::process`'s
/// `spawn_tree`, as the local host runs the CLI). Where it does not, it ends with the run.
pub fn detached_leaves_the_tree() -> bool {
    Os::LEAVES_AN_ENCLOSING_TREE
}

pub fn detach(command: Command) -> io::Result<Box<dyn ChildWrapper>> {
    let mut command = CommandWrap::from(command);
    Os::isolate(&mut command);
    command.spawn()
}
