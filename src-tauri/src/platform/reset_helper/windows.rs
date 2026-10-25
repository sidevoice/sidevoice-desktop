//! Cleanup is unavailable here; retain the authenticated wait and release without deleting anything.
use super::ResetHelper;
use crate::platform::reset_handoff::{DesktopReset, ResetHandoff};

pub struct DesktopResetHelper;

impl ResetHelper for DesktopResetHelper {
    fn run() {
        let Ok(helper) = DesktopReset::receive() else { return };
        let Ok(lock) = helper.await_exit() else { return };
        drop(lock);
    }
}
