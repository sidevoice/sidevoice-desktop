//! Not a phone: no system interruptions to observe (a build for tests).
use super::Os;

pub struct Native;

impl Os for Native {}
