//! Windows: while the workstation is locked, the input desktop is the secure one, which a process may not open.

use super::Os;
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_SWITCHDESKTOP};

pub(super) struct Native;

impl Os for Native {
    fn screen_locked() -> bool {
        // SAFETY: opens and closes a handle to the input desktop; nothing is kept.
        unsafe {
            let desktop = OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP);
            if desktop.is_null() {
                return true;
            }
            CloseDesktop(desktop);
            false
        }
    }
}
