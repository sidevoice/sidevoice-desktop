use super::Os;
use tauri::{AppHandle, Manager, Runtime};

pub(super) struct Native;

impl Os for Native {
    fn held<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
        app.try_state::<crate::voice::VoiceState>().map(|state| state.held())
    }
}
