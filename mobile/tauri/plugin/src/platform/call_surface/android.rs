use super::Os;

/// The notification is the app's own, drawn by the call's service: its stills stay in the app's files, and nothing else
/// draws the call.
pub struct Native;

impl Os for Native {}
