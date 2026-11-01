//! Keeping the screen on (docs/MOBILE.md → "Downloads"): the system suspends the app, and the downloads in it, soon after
//! the screen turns off. Each OS file implements `Os`; the common code is `crate::screen`.
use tauri::{AppHandle, Runtime};

use crate::ScreenError;

#[cfg_attr(target_os = "android", path = "screen/android.rs")]
#[cfg_attr(target_os = "ios", path = "screen/ios.rs")]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    path = "screen/other.rs"
)]
mod imp;
use imp::Native;

trait Os {
    /// Keeps the screen from turning off by itself (`on`), or lets it again.
    fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError>;
}

pub fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError> {
    Native::keep_on(app, on)
}
