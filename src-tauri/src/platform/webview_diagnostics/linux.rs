//! X11 and Wayland have no WebKitGTK diagnostic binding implemented; the empty availability list and rejecting `apply` are the defaults.

use super::Native;

pub(super) struct Os;

impl Native for Os {}
