//! What the phone's outputs are (docs/MOBILE.md → "The outputs' names"). Each OS file implements `Os`; the common code is
//! `crate::audio_outputs`.
use tauri::{AppHandle, Runtime};

use crate::Output;

#[cfg_attr(target_os = "android", path = "audio_outputs/android.rs")]
#[cfg_attr(target_os = "ios", path = "audio_outputs/ios.rs")]
mod imp;
use imp::Native;

trait Os {
    /// The phone's outputs, each by the system's id and its kind; `None` when the system does not answer; empty where the
    /// system names them already or where the output is not chosen from the app.
    fn outputs<R: Runtime>(app: &AppHandle<R>) -> Option<Vec<Output>>;
}

pub fn outputs<R: Runtime>(app: &AppHandle<R>) -> Option<Vec<Output>> {
    Native::outputs(app)
}
