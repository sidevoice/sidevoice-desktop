//! The phone's own outputs, named by what they are (docs/MOBILE.md → "The outputs' names"). Each OS file implements `Os`; the
//! common code is `crate::audio_outputs`.
use tauri::{AppHandle, Runtime};

use crate::BuiltInOutput;

#[cfg_attr(target_os = "android", path = "audio_outputs/android.rs")]
#[cfg_attr(target_os = "ios", path = "audio_outputs/ios.rs")]
mod imp;
use imp::Native;

trait Os {
    /// The phone's built-in outputs, each by the system's id and its name in the person's language; empty where the
    /// system names them already or where the output is not chosen from the app.
    fn built_in<R: Runtime>(app: &AppHandle<R>) -> Vec<BuiltInOutput>;
}

pub fn built_in<R: Runtime>(app: &AppHandle<R>) -> Vec<BuiltInOutput> {
    Native::built_in(app)
}
