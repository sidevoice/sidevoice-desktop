//! The phone's own log, where `log show`, a sysdiagnose and `adb logcat` read the app's lines: `warn` and `error` of our own
//! targets only, masked at the source like the app's file (the logging guideline, section 8; `sidevoice_shared::system_log`
//! is the rule and decides what goes in). iOS: the unified log, one subsystem and one category, through `tracing-oslog`.
//! Android: logcat under one fixed tag. Neither reads `SIDEVOICE_LOG`.
#[cfg_attr(target_os = "android", path = "system_log/android.rs")]
#[cfg_attr(target_os = "ios", path = "system_log/ios.rs")]
mod imp;
use imp::Native;

use sidevoice_shared::logging::Home;
use tracing_subscriber::{Layer, Registry};

/// What differs per OS.
trait Os {
    /// The layer that writes our lines to the system's log; `home` is shown as `~` where the layer writes the text itself.
    fn layer(home: Home) -> Box<dyn Layer<Registry> + Send + Sync>;
}

/// The layer for the system's log, to go under `sidevoice_shared::system_log::filter`.
pub fn layer(home: Home) -> Box<dyn Layer<Registry> + Send + Sync> {
    Native::layer(home)
}
