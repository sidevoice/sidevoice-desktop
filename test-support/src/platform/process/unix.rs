use super::Isolation;
use process_wrap::std::{CommandWrap, ProcessSession};

pub struct Os;

impl Isolation for Os {
    /// A session of its own is another process group: killing the run's group does not reach it.
    const LEAVES_AN_ENCLOSING_TREE: bool = true;

    fn isolate(command: &mut CommandWrap) {
        command.wrap(ProcessSession);
    }
}
