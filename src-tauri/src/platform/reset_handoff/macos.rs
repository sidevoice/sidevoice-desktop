//! macOS: the open files of an exiting app never keep it from removal; the handoff's own proof stands.

use super::Os;

pub(super) struct Native;

impl Os for Native {
    // The launch macOS shipped with: `current_exe`, unchecked. Tauri's resolver refuses an executable path with any
    // link in it, which used to work; tightening macOS is the operator's decision (see the PR).
    const PROGRAM_CHECKED: bool = false;

    // Nothing is held here.
    type Held = ();
}
