//! What differs by OS in the system's call: Android has a foreground service (Kotlin, `android/`) that adds a self-managed
//! call to Telecom, iOS a CallKit call; no other OS runs the mobile app (the crate exists only for a phone:
//! `#![cfg(mobile)]`). Each OS file implements `Os`; the common code is `crate::call_service`.
use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::{CallDetails, CallEvents, CallServiceError};

#[cfg_attr(target_os = "android", path = "call_service/android.rs")]
#[cfg_attr(target_os = "ios", path = "call_service/ios.rs")]
mod imp;
use imp::Native;

trait Os {
    /// Whether the system activates its call's audio session (and the app must leave that to it).
    const ACTIVATES_AUDIO: bool = false;

    /// Whether the system's call shows on the lock screen and takes a headset's buttons in place of the app's media
    /// controls.
    const REPLACES_MEDIA_CONTROLS: bool = false;

    /// What the plugin sets up when the app starts: on Android, the Kotlin plugin the service is reached through; on iOS,
    /// the app's CallKit provider.
    fn register<R: Runtime, C: DeserializeOwned>(
        api: PluginApi<R, C>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    fn start<R: Runtime>(
        app: &AppHandle<R>,
        details: &CallDetails,
        events: CallEvents,
    ) -> Result<(), CallServiceError>;

    fn update<R: Runtime>(
        app: &AppHandle<R>,
        details: &CallDetails,
    ) -> Result<(), CallServiceError>;

    fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError>;

    /// Asks the service to request the audio focus again; the answer comes as a focus event.
    fn refocus<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError>;
}

pub fn register<R: Runtime, C: DeserializeOwned>(
    api: PluginApi<R, C>,
) -> Result<(), Box<dyn std::error::Error>> {
    Native::register(api)
}

pub fn start<R: Runtime>(
    app: &AppHandle<R>,
    details: &CallDetails,
    events: CallEvents,
) -> Result<(), CallServiceError> {
    Native::start(app, details, events)
}

pub fn update<R: Runtime>(
    app: &AppHandle<R>,
    details: &CallDetails,
) -> Result<(), CallServiceError> {
    Native::update(app, details)
}

pub fn stop<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
    Native::stop(app)
}

pub fn refocus<R: Runtime>(app: &AppHandle<R>) -> Result<(), CallServiceError> {
    Native::refocus(app)
}

pub fn activates_audio() -> bool {
    Native::ACTIVATES_AUDIO
}

pub fn replaces_media_controls() -> bool {
    Native::REPLACES_MEDIA_CONTROLS
}
