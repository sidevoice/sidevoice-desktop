//! Removing the app itself from this computer, where the person can restore it from. The reset transaction in
//! `data.rs` runs [`uninstall`] in the reset helper after the app's data is deleted (`app_data`), and keeps the
//! decisions (report, relaunch).

#[cfg(target_os = "macos")]
#[path = "uninstall/macos.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "uninstall/linux.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "uninstall/windows.rs"]
mod imp;
use imp::Native;

use crate::platform::app_data::Error;

/// Whether this host can remove the app itself. Callers refuse an uninstall before its confirmation when false.
pub fn available() -> bool {
    Native::AVAILABLE
}

/// Remove the app itself where the person can restore it from (macOS: the Trash), checking first that the running
/// copy is one it may remove (macOS: a real bundle in a folder this user may write, not a disk image or a
/// translocated copy). Run after `app_data::delete_data`.
pub fn uninstall() -> Result<(), Error> {
    Native::uninstall()
}

/// What differs per OS. Hosts that cannot remove the app keep the defaults.
trait Os {
    /// Whether this host can remove the app itself.
    const AVAILABLE: bool = false;

    /// Remove the app itself.
    fn uninstall() -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test binary is never an installed copy: no host removes it, and a host without the capability says so.
    #[test]
    fn a_test_binary_is_never_removed() {
        match uninstall() {
            Ok(()) => panic!("the test binary was removed"),
            Err(Error::Unsupported) => assert!(!available()),
            Err(Error::Failed(key)) => assert!(available() && key == "reset.app-unremovable"),
            Err(Error::Stopped) => panic!("an uninstall has no steps to stop"),
        }
    }
}
