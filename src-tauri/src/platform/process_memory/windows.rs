//! Only the portable resident-memory observation is available here.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
