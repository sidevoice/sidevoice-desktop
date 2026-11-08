//! Android: no player of its own yet; the page plays its cues in its own Web Audio (the foreground service keeps the
//! webview's audio, which is not the case of iOS's native call session).
use super::Os;
use crate::Cue;

pub struct Native;

impl Os for Native {
    fn available() -> bool {
        false
    }

    fn decode(_: &[u8]) -> Result<(Vec<f32>, u32), String> {
        Err("cue-unavailable".into())
    }

    fn start(_: Cue, _: &str, _: &[u8]) -> Result<(), String> {
        Err("cue-unavailable".into())
    }

    fn stop(_: Cue) {}
}
