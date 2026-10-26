//! Linux has no microphone consent prompt and no settings URI: whether an input device exists is the shared rule's.
use super::{AccessState, Os};

pub struct Native;
impl Os for Native {
    fn permission() -> Result<AccessState, &'static str> {
        Ok(AccessState::Granted)
    }
}
