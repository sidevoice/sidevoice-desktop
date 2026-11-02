//! Android: the call's service holds the audio focus and reports its changes (`CallEvent::Focus`), so the app observes
//! nothing of its own.
use super::Os;

pub struct Native;

impl Os for Native {}
