//! Windows: the app does not remove itself; the trait's defaults.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
