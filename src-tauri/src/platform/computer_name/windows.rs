use super::Os;

pub(super) struct Native;

impl Os for Native {
    fn host_name() -> Option<String> {
        whoami::hostname().ok()
    }
}
