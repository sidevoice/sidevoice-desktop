//! The phone's own log, where `log show`, a sysdiagnose and `adb logcat` read the app's lines (`system_log::SystemLog` chooses
//! what goes in which category). iOS: the unified log through the Swift side (`SidevoiceLog`, which keeps every category but
//! the public four private). Android: logcat under one fixed tag, which has no privacy levels: only what the log allows is in it.
#[cfg_attr(target_os = "android", path = "system_log/android.rs")]
#[cfg_attr(target_os = "ios", path = "system_log/ios.rs")]
mod imp;
use imp::Native;

use tracing::Level;

/// What differs per OS.
trait Os {
    /// Writes `line` for the system's log, in `category` (iOS) at `level`.
    fn write(category: &str, level: Level, line: &str);
}

/// Writes one line of the app's log to the system's.
pub fn write(category: &str, level: Level, line: &str) {
    Native::write(category, level, line);
}
