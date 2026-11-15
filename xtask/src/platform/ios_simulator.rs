//! The iOS app run on a simulator, to see what its screen really shows. Apple's tool for it is part of Xcode.

use std::path::Path;

use crate::platform::host;
use crate::Result;

/// A host's way to run the app on a simulator. A host without Xcode says so.
trait Os {
    /// The simulator app `zip` (the `.app` zipped) installed on the newest iPhone Pro Max simulator and run, with bundle
    /// identifier `bundle`, in light and in dark appearance: in `out`, a screenshot of each (`screen-light.png`,
    /// `screen-dark.png`), the device used, the app's own log (`logs/`) and the system log of its process. Fails if the
    /// app does not launch or is not running when the screenshot is taken.
    fn screenshots(_zip: &Path, _bundle: &str, _out: &Path) -> Result<()> {
        Err(format!("the iOS simulator is run from a Mac with Xcode, not from {}", host::name()))
    }

    /// The Swift package `package`'s tests run on the newest iPhone Pro Max simulator, with `CALL_ACTIVITY_RENDERS` set to
    /// `out`, where the render tests leave their PNGs. Fails if a test fails.
    fn renders(_package: &Path, _out: &Path) -> Result<()> {
        Err(format!("the iOS simulator is run from a Mac with Xcode, not from {}", host::name()))
    }

    /// The Live Activity harness `harness` (an XcodeGen spec, `project.yml`, whose scheme `LiveActivityHarness` holds
    /// UI tests) built once and its tests run on the newest iPhone Pro simulator (not Pro Max), in light and then in
    /// dark appearance, with `LA_OUT` set to `out` and `LA_APPEARANCE` to the appearance (`LA_ONLY`, when given, passed
    /// on). In `out`: what the tests leave, `device.txt` and the harness's system log (`system-log.txt`). Both
    /// appearances run whatever the first did; fails if either failed.
    fn live_activity(_harness: &Path, _out: &Path, _only: Option<&str>) -> Result<()> {
        Err(format!("the iOS simulator is run from a Mac with Xcode, not from {}", host::name()))
    }
}

#[cfg_attr(target_os = "macos", path = "ios_simulator/macos.rs")]
#[cfg_attr(target_os = "windows", path = "ios_simulator/windows.rs")]
#[cfg_attr(target_os = "linux", path = "ios_simulator/linux.rs")]
mod imp;

use imp::Native;

/// Runs the tests of the Swift package `package` on a simulator; their renders go to `out` (see `Os::renders`).
pub fn renders(package: &Path, out: &Path) -> Result<()> {
    Native::renders(package, out)
}

/// Runs the Live Activity harness's UI tests on a simulator, in both appearances (see `Os::live_activity`).
pub fn live_activity(harness: &Path, out: &Path, only: Option<&str>) -> Result<()> {
    Native::live_activity(harness, out, only)
}

/// Runs the simulator app `zip` on a simulator and leaves its screenshots and logs in `out` (see `Os::screenshots`).
pub fn screenshots(zip: &Path, bundle: &str, out: &Path) -> Result<()> {
    Native::screenshots(zip, bundle, out)
}
