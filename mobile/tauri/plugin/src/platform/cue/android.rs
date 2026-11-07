//! Android: no player of its own yet; the page plays its cues in its own Web Audio (the foreground service keeps the
//! webview's audio, which is not the case of iOS's native call session).
use super::Os;
use crate::Cue;

pub struct Native;

impl Os for Native {
    fn available() -> bool {
        false
    }

    fn start(_: Cue, _: &[u8]) -> Result<(), String> {
        Err("cue-unavailable".into())
    }

    fn stop(_: Cue) {}
}
