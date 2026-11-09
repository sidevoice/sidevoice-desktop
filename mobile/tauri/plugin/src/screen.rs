//! The screen: kept on while the app needs it (a model download: the system suspends the app soon after the screen turns
//! off; iOS sets `isIdleTimerDisabled`, Android sets `FLAG_KEEP_SCREEN_ON` on the activity's window), and whether it is
//! locked (`host.screen.locked()`, docs/BRIDGE.md → "The screen": iOS's protected data, Android's keyguard or a screen off).
use tauri::{AppHandle, Runtime};

use crate::platform::screen as platform;

/// Why the screen could not be kept on, or let go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenError(pub String);

impl std::fmt::Display for ScreenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScreenError {}

/// Keeps the screen from turning off by itself (`on`), or lets it turn off again. Setting it twice is the same as once.
pub fn keep_screen_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError> {
    platform::keep_on(app, on)
}

/// Whether the person's screen is locked now, a screen that is off counting as locked; `false` where the system says nothing.
/// It waits for the system: call it off the main thread (the app's command does, `screen_locked`).
pub fn screen_locked<R: Runtime>(app: &AppHandle<R>) -> bool {
    platform::locked(app)
}
