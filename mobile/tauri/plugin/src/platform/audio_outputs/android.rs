use serde::Deserialize;
use tauri::{AppHandle, Runtime};

use super::Os;
use crate::platform::kotlin;
use crate::Output;

#[derive(Deserialize)]
struct Answer {
    outputs: Vec<Output>,
}

/// `AudioManager.getDevices`, read by the Kotlin plugin: Android names a built-in output by the phone's model.
pub struct Native;

impl Os for Native {
    fn outputs<R: Runtime>(app: &AppHandle<R>) -> Option<Vec<Output>> {
        match kotlin::ask::<_, Answer>(app, "audioOutputs", ())? {
            Ok(answer) => Some(answer.outputs),
            Err(error) => {
                log::warn!("audio outputs: the phone's outputs were not read: {error}");
                None
            }
        }
    }
}
