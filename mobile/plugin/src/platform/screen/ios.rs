use objc2::MainThreadMarker;
use objc2_ui_kit::UIApplication;
use tauri::{AppHandle, Runtime};

use super::Os;
use crate::ScreenError;

/// `UIApplication.isIdleTimerDisabled`, which UIKit wants set from the main thread.
pub struct Native;

impl Os for Native {
    fn keep_on<R: Runtime>(app: &AppHandle<R>, on: bool) -> Result<(), ScreenError> {
        app.run_on_main_thread(move || {
            if let Some(main) = MainThreadMarker::new() {
                UIApplication::sharedApplication(main).setIdleTimerDisabled(on);
            }
        })
        .map_err(|error| ScreenError(error.to_string()))
    }
}
