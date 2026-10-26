use super::Native;

pub(super) struct Os;

impl Native for Os {
    const TRAY_OPTIONAL: bool = true;
}
