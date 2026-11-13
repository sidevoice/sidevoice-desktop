//! Android: the call's service and Telecom route the call (the loudspeaker by default); nothing is moved here.
use super::Os;

pub struct Native;

impl Os for Native {
    fn on_the_phone() -> bool {
        false
    }

    fn force() -> Result<(), String> {
        Ok(())
    }
}
