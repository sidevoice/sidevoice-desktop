//! The app's updates (docs/BRIDGE.md → "The updater"): what the room page reads and sets. The shell
//! (`src/updater.rs`) drives tauri-plugin-updater; these are its pure parts.

use serde::{Deserialize, Serialize};
use std::time::Duration;

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

/// The last attempt that failed (`{code}`); the state stays what it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "code")]
pub enum ErrorCode {
    #[serde(rename = "update-check-failed")]
    CheckFailed,
    #[serde(rename = "update-download-failed")]
    DownloadFailed,
    #[serde(rename = "update-install-failed")]
    InstallFailed,
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
    pub error: Option<ErrorCode>,
}

impl Status {
    pub fn new(state: State, version: &str) -> Status {
        Status { state, version: Some(version.to_owned()), ..Status::default() }
    }

    pub fn failed(mut self, code: ErrorCode) -> Status {
        self.error = Some(code);
        self
    }
}

/// Which download may write, and the version the person stopped, both kept for this run of the app (not with the
/// settings: a new launch downloads that version again, automatic mode included). Every download holds a token from
/// `start`; `cancel` invalidates it at once, so a task that was aborted but is still running its last synchronous step
/// can no longer write progress, bytes or a result over the state of a replacement download.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Downloads {
    cancelled: Option<String>,
    generation: u64,
}

/// A download's claim to write: the generation `start` gave it.
pub type Token = u64;

impl Downloads {
    /// `host.updater.cancel()`: the status a download in progress returns to (`available`, the update still known),
    /// remembering its version; none when nothing is downloading.
    pub fn cancel(&mut self, status: &Status) -> Option<Status> {
        let version = status.version.as_deref().filter(|_| status.state == State::Downloading)?;
        self.cancelled = Some(version.to_owned());
        self.generation += 1;
        Some(Status::new(State::Available, version))
    }

    /// The token of a download of `version` that may start: when the person `asked` (which lifts the cancel) or it is
    /// not the one cancelled (a newer version always is); none otherwise.
    pub fn start(&mut self, version: &str, asked: bool) -> Option<Token> {
        if asked {
            self.cancelled = None;
        }
        (self.cancelled.as_deref() != Some(version)).then(|| {
            self.generation += 1;
            self.generation
        })
    }

    /// Whether `token` is still the download that may write: not cancelled, not replaced.
    pub fn owns(&self, token: Token) -> bool {
        self.generation == token
    }

    /// A download or an update ready that the feed has moved past is dropped: whatever was running owns nothing from
    /// now on (unlike `cancel`, no version is remembered as the person's refusal).
    pub fn supersede(&mut self) {
        self.generation += 1;
    }
}

/// Whether the feed now offering `offered` makes what is downloading or ready (`status`) stale: a different version.
/// A feed only ever moves on (nightlies are published one at a time, a release is `latest`), so installing the older
/// one would leave the app behind and offer the newer one again at the first check after the restart.
pub fn stale(status: &Status, offered: &str) -> bool {
    matches!(status.state, State::Downloading | State::Ready) && status.version.as_deref().is_some_and(|v| v != offered)
}

/// The releases' feed: `latest` is the newest release that is neither a draft nor a pre-release (never a `-rc.N`, never
/// the nightly).
const RELEASES_FEED: &str = "https://github.com/sidevoice/sidevoice-desktop/releases/latest/download/latest.json";
/// The nightly's own feed, beside its assets on the `nightly` pre-release (`cargo xtask publish assets nightly`).
const NIGHTLY_FEED: &str = "https://github.com/sidevoice/sidevoice-desktop/releases/download/nightly/latest.json";

/// The feed a build's updater reads, by the channel it was built for (RELEASING.md → "Updates"): a release the
/// releases', a nightly the nightly's, so each updates only to a newer build of its own channel. A dev build has none,
/// and no updater.
pub fn feed(channel: &str) -> Option<url::Url> {
    let feed = match channel {
        "release" => RELEASES_FEED,
        "nightly" => NIGHTLY_FEED,
        _ => return None,
    };
    Some(url::Url::parse(feed).expect("a feed is a URL"))
}

/// How long a build's updater waits after one check before the next, the first being at launch (RELEASING.md →
/// "Updates"): a nightly 10 minutes, as every green push to `main` makes one; a release an hour.
pub fn check_every(channel: &str) -> Duration {
    let minutes = if channel == "nightly" { 10 } else { 60 };
    Duration::from_secs(minutes * 60)
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
        assert_eq!(serde_json::to_value(ErrorCode::CheckFailed).unwrap(), json!({ "code": "update-check-failed" }));
        assert_eq!(
            serde_json::to_value(ErrorCode::DownloadFailed).unwrap(),
            json!({ "code": "update-download-failed" })
        );
    }

    #[test]
    fn cancelling_returns_a_download_to_available_and_nothing_else() {
        let mut cancelled = Downloads::default();
        let downloading = Status { progress: progress(42, Some(100)), ..Status::new(State::Downloading, "0.5.0") };
        assert_eq!(cancelled.cancel(&downloading), Some(Status::new(State::Available, "0.5.0")));
        for status in [Status::default(), Status::new(State::Available, "0.5.0"), Status::new(State::Ready, "0.5.0")] {
            assert_eq!(Downloads::default().cancel(&status), None, "{status:?}");
        }
        assert_eq!(cancelled.cancel(&Status::new(State::Available, "0.5.0")), None, "a second cancel is nothing");
    }

    #[test]
    fn a_cancelled_version_is_not_downloaded_again_by_itself_until_asked_or_a_newer_one_appears() {
        let mut cancelled = Downloads::default();
        assert!(cancelled.start("0.5.0", false).is_some(), "nothing cancelled yet");
        cancelled.cancel(&Status::new(State::Downloading, "0.5.0"));
        assert!(cancelled.start("0.5.0", false).is_none(), "automatic mode leaves it");
        assert!(cancelled.start("0.5.0", false).is_none(), "at every later check");
        assert!(cancelled.start("0.5.1", false).is_some(), "a newer version is downloaded");
        assert!(cancelled.start("0.5.0", true).is_some(), "the person asks");
        assert!(cancelled.start("0.5.0", false).is_some(), "and the cancel is lifted");
    }

    /// Cancel, then a replacement download (the same version asked again, or a newer one found): the aborted task's
    /// late progress or result is refused, the replacement's own is accepted.
    #[test]
    fn a_cancelled_download_cannot_write_over_its_replacement() {
        for (version, asked) in [("0.5.0", true), ("0.5.1", false)] {
            let mut downloads = Downloads::default();
            let old = downloads.start("0.5.0", false).unwrap();
            assert!(downloads.owns(old));
            downloads.cancel(&Status::new(State::Downloading, "0.5.0")).unwrap();
            assert!(!downloads.owns(old), "cancelled: it owns nothing at once");
            let new = downloads.start(version, asked).unwrap();
            assert!(!downloads.owns(old), "{version}: the replacement does not give it back");
            assert!(downloads.owns(new), "{version}");
            assert_ne!(old, new);
        }
        let mut downloads = Downloads::default();
        let first = downloads.start("0.5.0", false).unwrap();
        assert_eq!(downloads.cancel(&Status::new(State::Available, "0.5.0")), None);
        assert!(downloads.owns(first), "a cancel that cancels nothing leaves the download its own");
    }

    #[test]
    fn a_newer_version_on_the_feed_makes_a_download_or_a_ready_update_stale() {
        let ready = Status::new(State::Ready, "0.3.1-nightly.9");
        assert!(stale(&ready, "0.3.1-nightly.12"));
        assert!(stale(&Status::new(State::Downloading, "0.3.1-nightly.9"), "0.3.1-nightly.12"));
        assert!(!stale(&ready, "0.3.1-nightly.9"), "the same version is what is already there");
        for status in [Status::default(), Status::new(State::Available, "0.3.1-nightly.9")] {
            assert!(!stale(&status, "0.3.1-nightly.12"), "{status:?}: nothing downloaded to drop");
        }
    }

    #[test]
    fn superseding_takes_the_write_from_the_download_running() {
        let mut downloads = Downloads::default();
        let old = downloads.start("0.5.0", false).unwrap();
        downloads.supersede();
        assert!(!downloads.owns(old));
        assert!(downloads.start("0.5.0", false).is_some(), "no cancel is remembered");
    }

    #[test]
    fn each_channel_reads_its_own_feed_and_a_dev_build_none() {
        let feed = |channel| super::feed(channel).map(String::from);
        assert_eq!(
            feed("release").as_deref(),
            Some("https://github.com/sidevoice/sidevoice-desktop/releases/latest/download/latest.json")
        );
        assert_eq!(
            feed("nightly").as_deref(),
            Some("https://github.com/sidevoice/sidevoice-desktop/releases/download/nightly/latest.json")
        );
        for channel in ["dev", "", "beta"] {
            assert_eq!(feed(channel), None, "{channel:?}");
        }
    }

    #[test]
    fn a_nightly_checks_every_ten_minutes_and_a_release_every_hour() {
        assert_eq!(check_every("nightly"), Duration::from_secs(10 * 60));
        assert_eq!(check_every("release"), Duration::from_secs(60 * 60));
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
