//! Hearing keys the system gives no hot key for: a modifier or Fn on its own (macOS), and the app capturing the next key
//! itself while the person records a shortcut. Only here is the keyboard listened to, and only while a single key is set
//! (docs/SHORTCUTS.md → "Single keys"); a combination needs none of it.

use sidevoice_desktop_core::shortcuts::{Heard, Listened};
use std::sync::Arc;
use tauri::{AppHandle, Runtime};

#[cfg_attr(target_os = "macos", path = "key_capture/macos.rs")]
#[cfg_attr(target_os = "windows", path = "key_capture/windows.rs")]
#[cfg_attr(target_os = "linux", path = "key_capture/linux.rs")]
mod imp;
use imp::Native;

/// Where a listener's events go. Called on the listener's own thread: it must be quick.
pub type Sink = Arc<dyn Fn(Listened) + Send + Sync>;

/// Where a capture's events go. Called on the main thread.
pub type CaptureSink = Arc<dyn Fn(Heard) + Send + Sync>;

/// What differs per OS.
trait Os {
    /// Whether this system lets the app hear a modifier or Fn on its own (with the permission it asks, if any).
    const LISTENS: bool = false;

    /// Whether the app can capture the next key itself, with no permission: in its own window.
    const CAPTURES: bool = false;

    /// The system's page where the person turns the permission on, for one who refused it.
    const SETTINGS_URL: Option<&'static str> = None;

    /// Whether the system lets the app listen now. `None`: no permission concept here.
    fn access() -> Option<bool> {
        None
    }

    /// Asks the system for the permission (it shows its own prompt, once). Whether it is granted now.
    fn request_access() -> bool {
        false
    }

    /// Starts hearing keys. `None`: could not (no permission, no support).
    fn listen(_sink: Sink) -> Option<Box<dyn Send>> {
        None
    }

    /// Starts capturing the next keys, in the app's own window, swallowing them. `None`: could not.
    fn capture<R: Runtime>(_app: &AppHandle<R>, _sink: CaptureSink) -> Option<Box<dyn Send>> {
        None
    }
}

/// Whether a modifier or Fn on its own can be a shortcut here.
pub fn listens() -> bool {
    Native::LISTENS
}

/// Whether the app can capture the next key itself (the page then asks it to, `host.shortcuts.record`): Fn is not a key a
/// page can see.
pub fn captures() -> bool {
    Native::CAPTURES
}

/// The system's page where the person turns the permission on (macOS: Privacy & Security → Input Monitoring).
pub fn settings_url() -> Option<&'static str> {
    Native::SETTINGS_URL
}

/// Whether the system lets the app listen now; `None` where no permission is involved.
pub fn access() -> Option<bool> {
    Native::access()
}

/// Asks the system for the permission. Only on the person's own request: never at launch, never to find out.
pub fn request_access() -> bool {
    Native::request_access()
}

/// A listener on the keyboard: it listens until dropped.
pub struct Listener {
    _held: Box<dyn Send>,
}

pub fn listen(sink: Sink) -> Option<Listener> {
    Native::listen(sink).map(|_held| Listener { _held })
}

/// The app capturing keys: until dropped.
pub struct Capture {
    _held: Box<dyn Send>,
}

pub fn capture<R: Runtime>(app: &AppHandle<R>, sink: CaptureSink) -> Option<Capture> {
    Native::capture(app, sink).map(|_held| Capture { _held })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written against the interface: where a system cannot listen, it says so and listens to nothing.
    #[test]
    fn what_a_system_cannot_do_it_says() {
        if !listens() {
            assert!(listen(Arc::new(|_| {})).is_none());
            assert_eq!(access(), None);
            assert!(!request_access());
        }
        if !captures() {
            let app = tauri::test::mock_app();
            assert!(capture(app.handle(), Arc::new(|_| {})).is_none());
        }
    }
}
