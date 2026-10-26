use super::*;
use process_wrap::std::{CommandWrap, ProcessSession};

pub struct Os;
impl Detach for Os {
    fn detach(command: Command) -> io::Result<Box<dyn ChildWrapper>> {
        CommandWrap::from(command).wrap(ProcessSession).spawn()
    }
}
