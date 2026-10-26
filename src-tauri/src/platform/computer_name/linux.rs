use super::Os;
use sidevoice_desktop_core::computer_name;

pub(super) struct Native;

impl Os for Native {
    fn device_name() -> Option<String> {
        std::fs::read_to_string("/etc/machine-info").ok().and_then(|file| computer_name::pretty_hostname(&file))
    }

    fn host_name() -> Option<String> {
        whoami::hostname().ok()
    }
}
