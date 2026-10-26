//! Windows has no native media-control owner yet; the page retains Media Session.

use super::MediaControls;

pub struct NativeMediaControls;

impl MediaControls for NativeMediaControls {}
