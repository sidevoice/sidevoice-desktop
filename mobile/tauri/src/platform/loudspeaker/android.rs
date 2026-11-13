//! Android: the call's service and Telecom route the call (the loudspeaker by default); nothing is moved here.
use super::{Os, Output};

pub struct Native;

impl Os for Native {
    fn output() -> Output {
        Output::Other
    }

    fn force() -> Result<(), String> {
        Ok(())
    }
}
