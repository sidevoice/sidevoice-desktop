//! The global-shortcut plugin supplies registration here; no native label or reservation policy differs.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
