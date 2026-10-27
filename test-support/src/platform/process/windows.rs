use super::Isolation;
use process_wrap::std::{CommandWrap, CreationFlags};
use windows::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS};

pub struct Os;

impl Isolation for Os {
    /// The flags `sidevoice_platform::process::spawn_detached` (the connector's detached core) starts with: no console
    /// and a group of its own. It stays in an enclosing job, which does not let it break away.
    fn isolate(command: &mut CommandWrap) {
        command.wrap(CreationFlags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW));
    }
}
