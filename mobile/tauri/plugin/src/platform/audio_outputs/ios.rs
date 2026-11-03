use tauri::{AppHandle, Runtime};

use super::Os;
use crate::BuiltInOutput;

/// The route is the system's on iOS (`outputSelectable` is false): no output is listed for the person to read.
pub struct Native;

impl Os for Native {
    fn built_in<R: Runtime>(_app: &AppHandle<R>) -> Vec<BuiltInOutput> {
        Vec::new()
    }
}
