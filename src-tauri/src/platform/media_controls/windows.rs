//! Windows has no native media-control owner yet; the page retains Media Session.

use super::Native;

pub(super) struct Os;

impl Native for Os {}
