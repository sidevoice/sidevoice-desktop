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
    /// Adds the `process-wrap` wrapper that takes the child out of the caller's session, group or job. None by default.
    fn isolate(_command: &mut CommandWrap) {}
}

/// Spawns `command` outside the caller's process group, job or session, as a service handed to an independent owner.
pub fn detach(command: Command) -> io::Result<Box<dyn ChildWrapper>> {
    let mut command = CommandWrap::from(command);
    Os::isolate(&mut command);
    command.spawn()
}
