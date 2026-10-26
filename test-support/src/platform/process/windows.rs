use super::*;
use process_wrap::std::CommandWrap;
use std::os::windows::process::CommandExt;

pub struct Os;
impl Detach for Os {
    fn detach(mut command: Command) -> io::Result<Box<dyn ChildWrapper>> {
        // CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW, as a service handed to an independent owner.
        command.creation_flags(0x0100_0000 | 0x0800_0000);
        CommandWrap::from(command).spawn()
    }
}
