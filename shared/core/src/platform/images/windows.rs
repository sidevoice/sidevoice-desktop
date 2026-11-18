//! On Windows the loader's bias is the module's base address.

pub struct Native;

impl super::Os for Native {
    const BIAS_IS_BASE: bool = true;
}
