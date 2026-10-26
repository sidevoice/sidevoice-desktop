//! X11 and Wayland have no WebKitGTK diagnostic binding implemented; the empty availability list and rejecting `apply` are the defaults.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
