//! Android: its microphones are the voice's own list (the route is not handled here).
use sidevoice_shared::app::SystemInputs;

use super::Os;

pub struct Native;

impl Os for Native {
    fn inputs() -> Option<SystemInputs> {
        None
    }

    fn prefer(_id: Option<&str>) -> Result<(), String> {
        Err("audio-device-unknown".into())
    }
}
