//! Linux: the open files of an exiting app never keep it from removal; the handoff's own proof stands.

use super::Os;

pub(super) struct Native;

impl Os for Native {
    // Nothing is held here.
    type Held = ();
}
