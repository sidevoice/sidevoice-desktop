//! Starting a fixture service as an independent owner's process.
use std::{io, process::Command};

use process_wrap::std::ChildWrapper;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::Os as Detached;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Os as Detached;

pub trait Detach {
    /// Spawn a service outside the caller's process group/job/session.
    fn detach(command: Command) -> io::Result<Box<dyn ChildWrapper>>;
}
