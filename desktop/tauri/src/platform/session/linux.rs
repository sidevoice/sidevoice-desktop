//! No observation: the desktops lock through their own services (logind's `LockedHint`, each shell's screensaver), which this
//! does not read, so a hold on Linux is let go by the minute's limit, a sleep or the call's end only.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
