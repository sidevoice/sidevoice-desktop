use super::Os;

pub(super) struct Native;

impl Os for Native {
    const TRAY_OPTIONAL: bool = true;
}
