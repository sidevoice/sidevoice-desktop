use super::*;
use sidevoice_platform::ipc::{Address, Endpoint};
use std::os::unix::process::CommandExt;
pub struct Os;
impl FixtureHost for Os {
    fn detach(command: &mut Command) -> io::Result<std::process::Child> {
        // SAFETY: setsid is async-signal-safe and only changes the newly forked child.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn()
    }
    fn forget(data: &Path) {
        for endpoint in [Endpoint::new(&data.join("core"), "local"), Endpoint::new(data, "connector")] {
            let path = std::path::PathBuf::from(endpoint.to_string());
            if let Some(parent) = path.parent().filter(|p| !p.starts_with(data)) {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }
}
