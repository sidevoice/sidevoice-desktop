use tauri::{AppHandle, Runtime};

use super::Os;
use crate::ScreenError;

/// An OS the mobile app does not run on (its crates build here only so that the repository's checks cover them).
pub struct Native;

impl Os for Native {
    fn keep_on<R: Runtime>(_app: &AppHandle<R>, _on: bool) -> Result<(), ScreenError> {
        Err(ScreenError("this OS has no screen to keep on".into()))
    }
}
