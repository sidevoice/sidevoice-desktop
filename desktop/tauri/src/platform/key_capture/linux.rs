//! No listener: an X11 grab is exact, so a modifier alone is not a hot key, and reading every key needs the `input` group
//! or raw input (not offered, docs/SHORTCUTS.md → "Single keys").

use super::Os;

pub(super) struct Native;

impl Os for Native {}
