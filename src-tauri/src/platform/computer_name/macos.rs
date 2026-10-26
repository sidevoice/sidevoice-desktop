use super::Native;

pub(super) struct Os;

impl Native for Os {
    fn device_name() -> Option<String> {
        whoami::devicename().ok()
    }
}
