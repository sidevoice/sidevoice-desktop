//! Where the call's surfaces read the creatures' stills from (`crate::call_surface`), and the surface the system draws
//! apart from the call's service: iOS shares a container with the widget extension that draws the Live Activity (the App
//! Group's) and starts, updates and ends that activity through the app's Swift (the CallActivity package); Android's
//! notification is drawn by the call's service itself, and has neither. Each OS file implements `Os`; the common code is
//! `crate::call_surface`.
use std::path::PathBuf;
use std::sync::Arc;

#[cfg_attr(target_os = "android", path = "call_surface/android.rs")]
#[cfg_attr(target_os = "ios", path = "call_surface/ios.rs")]
mod imp;
use imp::Native;

/// Where the actions asked for on the surface go, by their names (`skip`, `hang-up`, …), from any thread.
pub type OnAction = Arc<dyn Fn(&str) + Send + Sync>;

trait Os {
    /// The folder the app shares with its extensions, where the system gives it one.
    fn shared_container() -> Option<PathBuf> {
        None
    }

    /// Starts the surface for a call shown as `details` (the call's details as JSON), reporting what the person asks for
    /// on it to `on_action`. Nothing where the call's service draws the surface itself.
    fn start(_details: &str, _on_action: OnAction) {}

    /// Shows the call as `details` now. Nothing while none is started.
    fn show(_details: &str) {}

    /// Ends it, and any an earlier run of the app left.
    fn end() {}

    /// A line for the system's log (iOS: the unified log, subsystem `ai.sidevoice`, public strings), where the app's own
    /// log file is not read by `log show`. Nothing elsewhere.
    fn os_log(_category: &str, _message: &str) {}

    /// Whether the surface is on the system's screens now (iOS: a Live Activity that has been requested and is not over:
    /// it needs iOS 16.1, the person may have it off, and it starts only while the app is in front). False where none is.
    fn running() -> bool {
        false
    }
}

pub fn shared_container() -> Option<PathBuf> {
    Native::shared_container()
}

pub fn start(details: &str, on_action: OnAction) {
    Native::start(details, on_action)
}

pub fn show(details: &str) {
    Native::show(details)
}

pub fn end() {
    Native::end()
}

pub fn running() -> bool {
    Native::running()
}

pub fn os_log(category: &str, message: &str) {
    Native::os_log(category, message)
}
