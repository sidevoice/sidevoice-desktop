use super::Native;

pub(super) struct Os;

impl Native for Os {
    fn host_name() -> Option<String> {
        whoami::hostname().ok()
    }
}
