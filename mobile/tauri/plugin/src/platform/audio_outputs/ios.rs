use tauri::{AppHandle, Runtime};

use super::Os;
use crate::Output;

/// The route is the system's on iOS (`outputSelectable` is false): no output is listed for the person to read.
pub struct Native;

impl Os for Native {
    fn outputs<R: Runtime>(_app: &AppHandle<R>) -> Option<Vec<Output>> {
        Some(Vec::new())
    }
}
