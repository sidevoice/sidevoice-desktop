//! A computer (the crate's tests and checks): the verifier reads the system's certificates itself.
use super::Os;

pub struct Native;

impl Os for Native {
    fn init() -> Result<(), String> {
        Ok(())
    }
}
