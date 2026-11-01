use serde::Serialize;
use tauri::{AppHandle, Runtime};

use super::Os;
use crate::platform::kotlin;
use crate::ScreenError;

#[derive(Serialize)]
struct Args {
    on: bool,
}

/// `FLAG_KEEP_SCREEN_ON` on the activity's window, set by the Kotlin plugin on Android's UI thread.
pub struct Native;

impl Os for Native {
    fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError> {
        kotlin::run(app, "keepScreenOn", Args { on })
            .ok_or_else(|| ScreenError("the plugin is not set up".into()))?
            .map_err(|error| ScreenError(error.to_string()))
    }
}
