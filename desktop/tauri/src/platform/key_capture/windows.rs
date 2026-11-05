//! No listener: `RegisterHotKey` takes a key with its modifiers, so a modifier alone is not a hot key. A low-level hook would
//! hear it with no permission, but it is what keyloggers use and what antivirus heuristics look at (docs/SHORTCUTS.md →
//! "Single keys"), so it is not installed.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
