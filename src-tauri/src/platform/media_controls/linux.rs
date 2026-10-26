//! X11 and Wayland have no native media-control owner yet; the page retains Media Session.

use super::Os;

pub(super) struct Native;

impl Os for Native {}
