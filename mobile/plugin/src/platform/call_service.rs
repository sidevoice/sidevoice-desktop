//! What differs by OS in the call's service: Android has one (Kotlin, `android/`), iOS keeps a call alive with a
//! background mode that is configuration (mobile/Info.ios.plist), and no other OS runs the mobile app. Each OS file
//! implements `Os`; the common code is `crate::call_service`.
use serde::de::DeserializeOwned;
use tauri::{ipc::Channel, plugin::PluginApi, AppHandle, Runtime};

use crate::{CallEvent, CallServiceError};

#[cfg_attr(target_os = "android", path = "call_service/android.rs")]
#[cfg_attr(target_os = "ios", path = "call_service/ios.rs")]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    path = "call_service/other.rs"
)]
mod imp;
use imp::Native;

trait Os {
    /// What the plugin sets up when the app starts: on Android, the Kotlin plugin the service is reached through.
    fn register<R: Runtime, C: DeserializeOwned>(
        api: PluginApi<R, C>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn start<R: Runtime>(
        app: &AppHandle<R>,
        events: Channel<CallEvent>,
    ) -> Result<(), CallServiceError>;

    fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError>;
}

pub fn register<R: Runtime, C: DeserializeOwned>(
    api: PluginApi<R, C>,
) -> Result<(), Box<dyn std::error::Error>> {
    Native::register(api)
}

pub fn start<R: Runtime>(
    app: &AppHandle<R>,
    events: Channel<CallEvent>,
) -> Result<(), CallServiceError> {
    Native::start(app, events)
}

pub fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
    Native::stop(app)
}
