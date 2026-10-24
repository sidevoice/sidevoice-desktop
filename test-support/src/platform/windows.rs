use super::*;
use std::os::windows::process::CommandExt;
pub struct Os;
impl FixtureHost for Os {
    fn detach(command: &mut Command) -> io::Result<std::process::Child> {
        // CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW, as a service handed to an independent owner.
        command.creation_flags(0x0100_0000 | 0x0800_0000).spawn()
    }
}
