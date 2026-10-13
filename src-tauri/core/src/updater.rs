//! The app's updates (docs/BRIDGE.md → "The updater"): what the room page reads and sets. The shell
//! (`src/updater.rs`) drives tauri-plugin-updater; these are its pure parts.

use serde::{Deserialize, Serialize};

/// How updates arrive, kept with the app's settings and set only by the page (`host.updater.setMode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Checked, downloaded in the background, installed on quit or on the person's restart.
    #[default]
    Automatic,
    /// Checked only: the person downloads it.
    Manual,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Idle,
    Available,
    Downloading,
    Ready,
}

/// The last attempt that failed; the state stays what it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ErrorCode {
    #[serde(rename = "update-check-failed")]
    CheckFailed,
    #[serde(rename = "update-download-failed")]
    DownloadFailed,
    #[serde(rename = "update-install-failed")]
    InstallFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub code: ErrorCode,
}

/// `host.updater.status()`: `{state, version?, progress?, error?: {code}}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Status {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
}

impl Status {
    pub fn new(state: State, version: &str) -> Status {
        Status { state, version: Some(version.to_owned()), ..Status::default() }
    }

    pub fn failed(mut self, code: ErrorCode) -> Status {
        self.error = Some(Failure { code });
        self
    }
}

/// A download's progress, 0..1 in hundredths (so the page is told at most a hundred times); none while its size is
/// unknown.
pub fn progress(received: u64, total: Option<u64>) -> Option<f64> {
    let total = total.filter(|&t| t > 0)?;
    Some((received.min(total) * 100 / total) as f64 / 100.0)
}

/// Whether `exe` is a macOS app's executable (`….app/Contents/MacOS/<name>`): a bare binary's folder is not an app the
/// plugin may replace.
pub fn in_app_bundle(exe: &std::path::Path) -> bool {
    let mut up = exe.ancestors().skip(1).map(|p| p.file_name().unwrap_or_default().to_string_lossy());
    up.next().as_deref() == Some("MacOS")
        && up.next().as_deref() == Some("Contents")
        && up.next().is_some_and(|app| app.ends_with(".app"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_status_and_the_mode_read_as_the_seam_says() {
        assert_eq!(serde_json::to_value(Status::default()).unwrap(), json!({"state": "idle"}));
        let downloading = Status { progress: progress(42, Some(100)), ..Status::new(State::Downloading, "0.5.0") };
        assert_eq!(
            serde_json::to_value(downloading).unwrap(),
            json!({"state": "downloading", "version": "0.5.0", "progress": 0.42})
        );
        let failed = Status::new(State::Ready, "0.5.0").failed(ErrorCode::InstallFailed);
        assert_eq!(
            serde_json::to_value(failed).unwrap(),
            json!({"state": "ready", "version": "0.5.0", "error": {"code": "update-install-failed"}})
        );
        assert_eq!(serde_json::to_value(ErrorCode::CheckFailed).unwrap(), json!("update-check-failed"));
        assert_eq!(serde_json::to_value(ErrorCode::DownloadFailed).unwrap(), json!("update-download-failed"));
        assert_eq!(Mode::default(), Mode::Automatic);
        assert_eq!(serde_json::from_value::<Mode>(json!("manual")).unwrap(), Mode::Manual);
        assert!(serde_json::from_value::<Mode>(json!("auto")).is_err());
    }

    #[test]
    fn only_an_app_bundle_s_executable_is_replaceable() {
        use std::path::Path;
        assert!(in_app_bundle(Path::new("/Applications/Sidevoice.app/Contents/MacOS/sidevoice-desktop")));
        assert!(!in_app_bundle(Path::new("/Users/me/src/target/release/sidevoice-desktop")), "a bare binary");
        assert!(!in_app_bundle(Path::new("/Applications/Sidevoice/Contents/MacOS/sidevoice-desktop")));
        assert!(!in_app_bundle(Path::new("sidevoice-desktop")));
    }

    #[test]
    fn progress_is_in_hundredths_and_absent_without_a_size() {
        assert_eq!(progress(0, Some(1000)), Some(0.0));
        assert_eq!(progress(259, Some(1000)), Some(0.25));
        assert_eq!(progress(2000, Some(1000)), Some(1.0));
        assert_eq!(progress(10, None), None);
        assert_eq!(progress(10, Some(0)), None);
    }
}
