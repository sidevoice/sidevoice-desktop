use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

use super::Os;
use crate::platform::kotlin;
use crate::ScreenError;

#[derive(Serialize)]
struct Args {
    on: bool,
}

/// The Kotlin plugin's answer to `screenLocked`.
#[derive(Deserialize)]
struct Locked {
    locked: bool,
}

/// `FLAG_KEEP_SCREEN_ON` on the activity's window, set by the Kotlin plugin on Android's UI thread; the keyguard and the
/// power manager for the lock (`screenLocked`). No watch: Android has no native cue to gate on it (the page plays its own).
pub struct Native;

impl Os for Native {
    fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError> {
        kotlin::run(app, "keepScreenOn", Args { on })
            .ok_or_else(|| ScreenError("the plugin is not set up".into()))?
            .map_err(|error| ScreenError(error.to_string()))
    }

    /// `KeyguardManager.isKeyguardLocked` (the lock screen shows, a swipe one included) or not `PowerManager.isInteractive`
    /// (the screen is off, with no lock at all too).
    fn locked<R: Runtime>(app: &AppHandle<R>) -> bool {
        match kotlin::run::<_, Locked>(app, "screenLocked", ()) {
            Some(Ok(answer)) => answer.locked,
            Some(Err(_)) => {
                log::warn!("screen: whether it is locked, not answered");
                false
            }
            None => false,
        }
    }
}
