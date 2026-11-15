//! The screen stays on while a model download runs (sidevoice-desktop#361): the system suspends the app, and the download
//! in it, soon after the screen turns off, and a partial download is only resumed by the next attempt. The host tells when
//! the first download starts and the last one ends ([`DownloadActivity`]); the OS-specific part is the plugin's.
use sidevoice_mobile_plugin::keep_screen_on;
use sidevoice_shared::app::DownloadActivity;
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
        if keep_screen_on(&self.app, on).is_err() {
            tracing::warn!(
                keep = on,
                "the screen could not be kept on or released for a download"
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
