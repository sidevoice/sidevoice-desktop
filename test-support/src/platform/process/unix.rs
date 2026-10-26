use super::Isolation;
use process_wrap::std::{CommandWrap, ProcessSession};

pub struct Os;

impl Isolation for Os {
    fn isolate(command: &mut CommandWrap) {
        command.wrap(ProcessSession);
    }
}
