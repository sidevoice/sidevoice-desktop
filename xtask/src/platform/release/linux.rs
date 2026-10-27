//! Linux: no release step of its own (its installers come from `cargo xtask bundle`).

use super::Os;

pub struct Native;

impl Os for Native {}
