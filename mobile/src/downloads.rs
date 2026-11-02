//! The screen stays on while a model download runs (sidevoice-desktop#361): the system suspends the app, and the download
//! in it, soon after the screen turns off, and a partial download is only resumed by the next attempt. The host tells when
//! the first download starts and the last one ends ([`DownloadActivity`]); the OS-specific part is the plugin's.
use sidevoice_app_core::app::DownloadActivity;
use sidevoice_mobile_plugin::keep_screen_on;
use tauri::AppHandle;

/// Keeps the screen on for as long as the host has a download running.
pub struct KeepScreenOn {
    app: AppHandle,
}

impl KeepScreenOn {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn set(&self, on: bool) {
        if let Err(error) = keep_screen_on(&self.app, on) {
            log::warn!(
                "screen: could not {} the screen on: {error}",
                if on { "keep" } else { "release" }
            );
        }
    }
}

impl DownloadActivity for KeepScreenOn {
    fn started(&self) {
        self.set(true);
    }

    fn ended(&self) {
        self.set(false);
    }
}
