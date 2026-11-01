//! What this app gives sidevoice-app-core's application host (`AppHost`): its reset as the admission to work that uses
//! the app's data, and the namespace of its provider keys in the OS credential store. (Where the call's events go is the
//! shared plugin's: the channel the room page opens.)

use sidevoice_app_core::app::{Admission, Keychain, Lease};
use tauri::AppHandle;

/// Work is admitted while no reset is under way, and holds the reset off while it lasts (`data::Permit`).
pub(crate) struct ResetAdmission(pub(crate) AppHandle);

impl Admission for ResetAdmission {
    fn admit(&self) -> Option<Lease> {
        crate::data::admit(&self.0).map(Lease::new)
    }
}

/// The namespace of this app's provider keys in the OS credential store (docs/CREDENTIALS.md): the app's, and exactly
/// what it has always been, so the keys it holds are the keys it finds.
const PROVIDER_KEYS: &str = "ai.sidevoice.desktop.providers";

/// The provider keys of this app.
pub(crate) fn keychain() -> Keychain {
    Keychain::new(PROVIDER_KEYS)
}

/// The levels the room page reports during a call go to the call controls card: the microphone's for its wave, the
/// voice's for its avatar's mouth.
pub(crate) struct CardLevels(pub(crate) AppHandle);

impl sidevoice_app_tauri::CallObserver for CardLevels {
    fn level(&self, level: u8) {
        crate::call_controls::level(&self.0, level);
    }

    fn playback(&self, level: f64, thread_id: Option<String>) {
        crate::call_controls::playback(&self.0, level, thread_id);
    }
}
