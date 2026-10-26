use super::Native;
use tauri::{AppHandle, Manager, Runtime};

pub(super) struct Os;

impl Native for Os {
    fn held<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
        app.try_state::<crate::voice::VoiceState>().map(|state| state.held())
    }
}
