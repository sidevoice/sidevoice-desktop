use super::Os;

pub(super) struct Native;

impl Os for Native {
    fn device_name() -> Option<String> {
        whoami::devicename().ok()
    }
}
