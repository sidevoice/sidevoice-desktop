//! No native library whose exit destructors the probe observes runs here.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
