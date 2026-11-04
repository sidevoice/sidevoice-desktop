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
    /// Whether the system makes its call's audio ready (CallKit's session, Telecom's active call and route), and the
    /// service reports when (`CallEvent::Audio`).
    const ACTIVATES_AUDIO: bool = false;

    /// Whether the system's call shows on the lock screen and takes a headset's buttons in place of the app's media
    /// controls.
    const REPLACES_MEDIA_CONTROLS: bool = false;

    /// Whether the app's own call (not a system's call) still runs a service: Android's foreground service keeps the process
    /// alive while the screen is off. iOS has none: the `audio` background mode does it.
    const OWN_CALL_SERVICE: bool = false;

    /// What the plugin sets up when the app starts: on Android, the Kotlin plugin the service is reached through; on iOS,
    /// nothing (the CallKit provider is made by the first system's call).
    fn register<R: Runtime, C: DeserializeOwned>(
        api: PluginApi<R, C>,
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Starts the call; `system`: as a system's call (Telecom, CallKit), else as the app's own.
    fn start<R: Runtime>(
        app: &AppHandle<R>,
        details: &CallDetails,
        system: bool,
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

/// Starts the call. The app's own call (`system` false) runs only what the OS keeps for it ([`Os::OWN_CALL_SERVICE`]):
/// nothing where there is none, and no CallKit or Telecom object is made.
pub fn start<R: Runtime>(
    app: &AppHandle<R>,
    details: &CallDetails,
    system: bool,
    events: CallEvents,
) -> Result<(), CallServiceError> {
    if !system && !Native::OWN_CALL_SERVICE {
        return Ok(());
    }
    Native::start(app, details, system, events)
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

pub fn activates_audio(system: bool) -> bool {
    system && Native::ACTIVATES_AUDIO
}

pub fn replaces_media_controls(system: bool) -> bool {
    if system {
        Native::REPLACES_MEDIA_CONTROLS
    } else {
        // The app's own call: only while its own surface is on the screens or on its way (iOS: the Live Activity runs, or
        // the call's start asked for it), so that a Lock Screen with no surface keeps its Now Playing and the headset's
        // buttons, and a call with one never shows Now Playing, not even before the activity is requested.
        crate::call_surface::surface_running()
    }
}
