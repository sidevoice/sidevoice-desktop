//! Windows: nothing yet but the size of a process id. The Job Object a tree runs in, waiting on its pipes and asking
//! about a process come with the Windows local host; until then no program is run and no recorded process is alive.

use super::Os;

pub struct Native;

impl Os for Native {
    /// A `DWORD`.
    const MAX_PID: u64 = u32::MAX as u64;
}
