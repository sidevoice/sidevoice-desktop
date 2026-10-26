use super::VoiceHolds;
use tauri::{AppHandle, Manager, Runtime};

pub struct NativeVoiceHolds;

impl VoiceHolds for NativeVoiceHolds {
    fn held<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
        app.try_state::<crate::voice::VoiceState>().map(|state| state.held())
    }
}
