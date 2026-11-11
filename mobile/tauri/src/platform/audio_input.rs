//! The microphones the system's audio route offers and prefers (`InputRoute`, the shared host's seam for them). iOS keeps
//! the process's audio session: the inputs it offers (the phone's microphone, a car's hands-free, AirPods, a wired set) and
//! the one the person prefers, which the system routes the call's microphone from. Declaring a preferred input changes
//! neither the session's category, nor its mode, nor its options (the call stays a call: desktop#466). Android's route is
//! not handled here: its microphones stay the voice's own list.
use std::sync::Arc;

use sidevoice_shared::app::{InputRoute, SystemInputs};

#[cfg_attr(target_os = "android", path = "audio_input/android.rs")]
#[cfg_attr(target_os = "ios", path = "audio_input/ios.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// The inputs the system offers now and the one preferred; `None` where the OS has no route of its own for them.
    fn inputs() -> Option<SystemInputs>;
    /// Prefers input `id` (`None`: no preference). `audio-device-unknown` for an id the system does not offer.
    fn prefer(id: Option<&str>) -> Result<(), String>;
}

/// The app's route for the microphone.
pub fn route() -> Arc<dyn InputRoute> {
    Arc::new(System)
}

struct System;

impl InputRoute for System {
    fn list(&self) -> Option<SystemInputs> {
        Native::inputs()
    }

    fn choose(&self, id: Option<&str>) -> Result<(), String> {
        Native::prefer(id)
    }
}
