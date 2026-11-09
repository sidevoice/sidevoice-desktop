//! The screen: keeping it on (docs/MOBILE.md → "Downloads": the system suspends the app, and the downloads in it, soon after
//! the screen turns off), and whether it is locked (docs/BRIDGE.md → "The screen": the thinking sound's "only with the screen
//! off"). Each OS file implements `Os`; the common code is `crate::screen` and `crate::cue`.
use tauri::{AppHandle, Runtime};

use crate::ScreenError;

#[cfg_attr(target_os = "android", path = "screen/android.rs")]
#[cfg_attr(target_os = "ios", path = "screen/ios.rs")]
mod imp;
use imp::Native;

trait Os {
    /// Keeps the screen from turning off by itself (`on`), or lets it again.
    fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError>;
    /// Whether the screen is locked now, a screen that is off counting as locked, as the system says it; `false` where it
    /// says nothing. Waits for the system's answer: never on Android's main thread, where that answer is given.
    fn locked<R: Runtime>(app: &AppHandle<R>) -> bool;
    /// From now on, `changed` hears on the main thread whether the screen is locked, each time the system says it changed or
    /// the OS reads it again. Once per app, as its plugins are set up: before the system's application may exist, so it reads
    /// nothing then. Nothing where the system tells nothing.
    fn watch<R: Runtime>(_app: &AppHandle<R>, _changed: fn(bool)) {}
}

pub fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError> {
    Native::keep_on(app, on)
}

pub fn locked<R: Runtime>(app: &AppHandle<R>) -> bool {
    Native::locked(app)
}

pub fn watch<R: Runtime>(app: &AppHandle<R>, changed: fn(bool)) {
    Native::watch(app, changed);
}
