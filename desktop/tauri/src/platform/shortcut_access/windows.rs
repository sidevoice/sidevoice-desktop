//! The global-shortcut plugin supplies registration here; no native label or reservation policy differs.

use super::Os;
use sidevoice_desktop_core::shortcuts::Platform;

pub(super) struct Native;

impl Os for Native {
    const PLATFORM: Platform = Platform::Windows;
}
