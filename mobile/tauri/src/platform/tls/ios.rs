//! iOS: the verifier asks the Security framework directly; there is nothing to hand it.
use super::Os;

pub struct Native;

impl Os for Native {
    fn init() -> Result<(), String> {
        Ok(())
    }
}
