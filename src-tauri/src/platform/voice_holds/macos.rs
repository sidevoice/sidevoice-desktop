use super::VoiceHolds;
use tauri::{AppHandle, Manager};

pub struct NativeVoiceHolds;

impl VoiceHolds for NativeVoiceHolds {
    fn held(app: &AppHandle) -> Option<String> {
        app.try_state::<crate::voice::VoiceState>().map(|state| state.held())
    }
}
