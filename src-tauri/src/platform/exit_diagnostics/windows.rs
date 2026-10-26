//! No native library whose exit destructors the probe observes runs here.

use super::Native;

pub(super) struct Os;

impl Native for Os {}
