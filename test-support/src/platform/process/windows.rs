use super::Isolation;
use process_wrap::std::{CommandWrap, CreationFlags};
use windows::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW};

pub struct Os;

impl Isolation for Os {
    fn isolate(command: &mut CommandWrap) {
        command.wrap(CreationFlags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW));
    }
}
