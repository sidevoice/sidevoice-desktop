use serde::Deserialize;
use tauri::{AppHandle, Runtime};

use super::Os;
use crate::platform::kotlin;
use crate::BuiltInOutput;

#[derive(Deserialize)]
struct Answer {
    outputs: Vec<BuiltInOutput>,
}

/// `AudioManager.getDevices`, read by the Kotlin plugin: Android names a built-in output by the phone's model.
pub struct Native;

impl Os for Native {
    fn built_in<R: Runtime>(app: &AppHandle<R>) -> Vec<BuiltInOutput> {
        match kotlin::ask::<_, Answer>(app, "builtInOutputs", ()) {
            Some(Ok(answer)) => answer.outputs,
            Some(Err(error)) => {
                log::warn!("audio outputs: the built-in ones are not named: {error}");
                Vec::new()
            }
            None => Vec::new(),
        }
    }
}
