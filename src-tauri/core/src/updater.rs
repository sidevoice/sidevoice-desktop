//! The app's updates (docs/BRIDGE.md → "The updater"): whether this install can update itself, and what to do next as
//! checks, downloads, the person's "Restart to update" and the call come and go.
//!
//! Pure decisions, no I/O: the shell (`src/updater.rs`) checks the release's `latest.json`, downloads and verifies the
//! update and installs it, and tells [`Updates`] how each went; [`Updates`] answers the next [`Step`] and the
//! [`Status`] the room page reads (`host.updater.status()`).
//!
//! - Automatic download on (the default): an update found is downloaded in the background, and the page is told
//!   `available` once it is downloaded and verified, so a restart never waits on the network.
//! - Off: the page is told `available` as soon as one is found; "Restart to update" then downloads it first.
//! - "Restart to update" installs and restarts at once outside a call; during one (the room's call joined, or the
//!   voice call the app runs listening), once the call has ended.

use std::path::Path;

/// How this copy of the app was installed, as the shell finds it (Tauri's bundle type, and where it runs from).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// A macOS `.app`; `in_bundle`: the executable runs from inside one, outside a read-only translocated copy.
    MacApp { in_bundle: bool },
    /// A Linux AppImage; `mounted`: run as one (its `APPIMAGE` path is set), so there is a file to replace.
    AppImage { mounted: bool },
    /// The Windows NSIS installer's.
    Nsis,
    /// A Linux `.deb`: the system's package manager owns it, so the app never replaces itself.
    Deb,
    /// Another bundle the release does not publish updates for (`.rpm`, `.msi`).
    Other,
    /// No bundle at all: `cargo run`, `tauri dev`, a bare binary.
    Unbundled,
}

/// Whether this copy can update itself, or why not (the shell logs it; the page then has no `host.updater`).
pub fn offered(debug_build: bool, install: Install) -> Result<(), &'static str> {
    if debug_build {
        return Err("a debug build");
    }
    match install {
        Install::MacApp { in_bundle: true } | Install::AppImage { mounted: true } | Install::Nsis => Ok(()),
        Install::MacApp { in_bundle: false } => Err("not running from an .app it can replace"),
        Install::AppImage { mounted: false } => Err("an AppImage's binary run on its own"),
        Install::Deb => Err("a .deb: the package manager updates it"),
        Install::Other => Err("a bundle the releases publish no updates for"),
        Install::Unbundled => Err("not a bundled app"),
    }
}

/// Whether `exe` is a macOS app's executable (`….app/Contents/MacOS/<name>`) that is not running from the read-only
/// copy Gatekeeper's App Translocation makes of an app opened where it was downloaded.
pub fn in_app_bundle(exe: &Path) -> bool {
    let Some(macos) = exe.parent() else { return false };
    let Some(contents) = macos.parent() else { return false };
    let Some(app) = contents.parent() else { return false };
    let named = |p: &Path, name: &str| p.file_name().is_some_and(|n| n == name);
    named(macos, "MacOS")
        && named(contents, "Contents")
        && app.extension().is_some_and(|e| e == "app")
        && !exe.components().any(|c| c.as_os_str() == "AppTranslocation")
}

/// What the room page reads (`host.updater.status()`): `{available, version?}`. `required` is never said: there is no
/// policy for it yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// What the shell does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Download and verify the update the last check announced, this version.
    Download(String),
    /// Install the update downloaded and verified, then restart into it.
    Install,
}

/// "Restart to update" with nothing to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NothingToInstall;

/// The updates' state: what the last check announced, what is downloaded, what is under way, and what the person asked.
#[derive(Debug, Clone)]
pub struct Updates {
    auto_download: bool,
    /// The version the last successful check announced (`None`: it found none newer than this app).
    found: Option<String>,
    /// The version downloaded and verified, ready to install.
    ready: Option<String>,
    downloading: Option<String>,
    /// The version whose download or install last failed: not downloaded again on its own until the next check.
    failed: Option<String>,
    /// The person asked to restart into the update.
    restart: bool,
    in_call: bool,
    installing: bool,
    checking: bool,
}

impl Updates {
    pub fn new(auto_download: bool) -> Self {
        Updates {
            auto_download,
            found: None,
            ready: None,
            downloading: None,
            failed: None,
            restart: false,
            in_call: false,
            installing: false,
            checking: false,
        }
    }

    pub fn status(&self) -> Status {
        // Off, the person is told of an update as soon as one is found; on, once it is downloaded.
        let version = match (&self.found, &self.ready) {
            (Some(found), _) if !self.auto_download => Some(found.clone()),
            (_, Some(ready)) => Some(ready.clone()),
            _ => None,
        };
        Status { available: version.is_some(), version }
    }

    /// A check is about to start: `false` when one already runs, or an install is under way.
    pub fn begin_check(&mut self) -> bool {
        if self.checking || self.installing {
            return false;
        }
        self.checking = true;
        true
    }

    /// How the check went: the newer version it announced, none, or a failure (the network, the endpoint, a manifest
    /// without this platform), which changes nothing.
    pub fn checked(&mut self, result: Result<Option<String>, ()>) -> Step {
        self.checking = false;
        match result {
            Ok(found) => {
                self.found = found;
                self.failed = None;
            }
            Err(()) => return Step::Nothing,
        }
        self.next()
    }

    /// How the download of `version` went (verified against the app's public key, or not).
    pub fn downloaded(&mut self, version: &str, ok: bool) -> Step {
        self.downloading = None;
        if ok {
            self.ready = Some(version.to_string());
        } else {
            self.failed = Some(version.to_string());
            // A restart that was waiting on it is not left pending: the person may ask again.
            self.restart = false;
        }
        self.next()
    }

    /// The install failed (the app is where it cannot be replaced, the disk): nothing is installed, the person may ask
    /// again after the next check.
    pub fn install_failed(&mut self) {
        self.installing = false;
        self.restart = false;
        self.failed = self.ready.take();
    }

    pub fn set_auto_download(&mut self, on: bool) -> Step {
        self.auto_download = on;
        self.next()
    }

    /// "Restart to update": installs and restarts now, or once the update is downloaded, or once the call has ended.
    pub fn restart(&mut self) -> Result<Step, NothingToInstall> {
        if !self.status().available {
            return Err(NothingToInstall);
        }
        self.restart = true;
        Ok(self.next())
    }

    /// Whether a call is on: the room's call joined, or the voice call the app runs listening.
    pub fn call(&mut self, in_call: bool) -> Step {
        self.in_call = in_call;
        self.next()
    }

    fn next(&mut self) -> Step {
        if self.installing {
            return Step::Nothing;
        }
        // The version the person would be told of: off, the one found; on, the one found once it is downloaded.
        let wanted = match (&self.found, self.auto_download) {
            (Some(found), false) if self.restart => Some(found.clone()),
            (Some(found), true) if self.failed.as_ref() != Some(found) => Some(found.clone()),
            _ => None,
        };
        if let Some(version) = wanted {
            if self.ready.as_ref() != Some(&version) && self.downloading.is_none() {
                self.downloading = Some(version.clone());
                return Step::Download(version);
            }
        }
        // Off, a restart waits for the version it was told of; on, it installs what is downloaded.
        let waiting = !self.auto_download && self.found.is_some() && self.ready != self.found;
        if self.restart && self.ready.is_some() && !waiting && !self.in_call {
            self.installing = true;
            return Step::Install;
        }
        Step::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(v: &str) -> Result<Option<String>, ()> {
        Ok(Some(v.to_string()))
    }

    fn available(v: &str) -> Status {
        Status { available: true, version: Some(v.into()) }
    }

    #[test]
    fn only_a_release_bundle_that_can_replace_itself_updates() {
        assert_eq!(offered(false, Install::MacApp { in_bundle: true }), Ok(()));
        assert_eq!(offered(false, Install::AppImage { mounted: true }), Ok(()));
        assert_eq!(offered(false, Install::Nsis), Ok(()));
        assert!(offered(true, Install::Nsis).is_err(), "a debug build never updates");
        assert!(offered(false, Install::Deb).is_err(), "a .deb is the package manager's");
        assert!(offered(false, Install::AppImage { mounted: false }).is_err());
        assert!(offered(false, Install::MacApp { in_bundle: false }).is_err());
        assert!(offered(false, Install::Other).is_err());
        assert!(offered(false, Install::Unbundled).is_err(), "cargo run, tauri dev");
    }

    #[test]
    fn a_mac_app_runs_from_its_bundle() {
        assert!(in_app_bundle(Path::new("/Applications/Sidevoice.app/Contents/MacOS/sidevoice-desktop")));
        assert!(!in_app_bundle(Path::new("/Users/me/src/target/release/sidevoice-desktop")));
        assert!(!in_app_bundle(Path::new("/Applications/Sidevoice/Contents/MacOS/sidevoice-desktop")));
        assert!(!in_app_bundle(Path::new("sidevoice-desktop")));
        assert!(!in_app_bundle(Path::new(
            "/private/var/folders/x/T/AppTranslocation/ABC/d/Sidevoice.app/Contents/MacOS/sidevoice-desktop"
        )));
    }

    #[test]
    fn on_an_update_is_available_once_downloaded() {
        let mut u = Updates::new(true);
        assert_eq!(u.status(), Status::default());
        assert!(u.begin_check());
        assert!(!u.begin_check(), "one check at a time");
        assert_eq!(u.checked(found("0.3.0")), Step::Download("0.3.0".into()));
        assert_eq!(u.status(), Status::default(), "not before it is downloaded");
        assert_eq!(u.downloaded("0.3.0", true), Step::Nothing);
        assert_eq!(u.status(), available("0.3.0"));
        assert_eq!(
            serde_json::to_value(u.status()).unwrap(),
            serde_json::json!({"available": true, "version": "0.3.0"})
        );
        // The next check finds the same: nothing to download again.
        assert!(u.begin_check());
        assert_eq!(u.checked(found("0.3.0")), Step::Nothing);
    }

    #[test]
    fn nothing_found_or_a_failed_check_says_nothing_available() {
        let mut u = Updates::new(true);
        assert!(u.begin_check());
        assert_eq!(u.checked(Ok(None)), Step::Nothing);
        assert_eq!(u.status(), Status::default());
        assert_eq!(serde_json::to_value(u.status()).unwrap(), serde_json::json!({"available": false}));
        // No endpoint reachable, no latest.json yet, a manifest without this platform.
        assert!(u.begin_check());
        assert_eq!(u.checked(Err(())), Step::Nothing);
        assert_eq!(u.status(), Status::default());
        assert_eq!(u.restart(), Err(NothingToInstall));
    }

    #[test]
    fn a_failed_check_keeps_what_was_downloaded() {
        let mut u = Updates::new(true);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.downloaded("0.3.0", true);
        u.begin_check();
        assert_eq!(u.checked(Err(())), Step::Nothing);
        assert_eq!(u.status(), available("0.3.0"));
    }

    #[test]
    fn a_failed_download_waits_for_the_next_check() {
        let mut u = Updates::new(true);
        u.begin_check();
        assert_eq!(u.checked(found("0.3.0")), Step::Download("0.3.0".into()));
        assert_eq!(u.downloaded("0.3.0", false), Step::Nothing, "not again at once: a bad signature would loop");
        assert_eq!(u.status(), Status::default());
        u.begin_check();
        assert_eq!(u.checked(found("0.3.0")), Step::Download("0.3.0".into()));
    }

    #[test]
    fn a_newer_version_found_while_one_is_ready_is_downloaded_too() {
        let mut u = Updates::new(true);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.downloaded("0.3.0", true);
        u.begin_check();
        assert_eq!(u.checked(found("0.4.0")), Step::Download("0.4.0".into()));
        assert_eq!(u.status(), available("0.3.0"), "the one ready stays offered meanwhile");
        u.downloaded("0.4.0", true);
        assert_eq!(u.status(), available("0.4.0"));
    }

    #[test]
    fn restart_outside_a_call_installs_at_once() {
        let mut u = Updates::new(true);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.downloaded("0.3.0", true);
        assert_eq!(u.restart(), Ok(Step::Install));
        assert!(!u.begin_check(), "no check while installing");
        assert_eq!(u.restart(), Ok(Step::Nothing), "one install");
    }

    #[test]
    fn restart_during_a_call_waits_for_it_to_end() {
        let mut u = Updates::new(true);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.downloaded("0.3.0", true);
        assert_eq!(u.call(true), Step::Nothing);
        assert_eq!(u.restart(), Ok(Step::Nothing), "not mid-call");
        assert_eq!(u.call(true), Step::Nothing, "still in it");
        assert_eq!(u.call(false), Step::Install, "the call ended");
    }

    #[test]
    fn a_call_without_a_restart_installs_nothing() {
        let mut u = Updates::new(true);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.downloaded("0.3.0", true);
        u.call(true);
        assert_eq!(u.call(false), Step::Nothing, "only the person's restart installs");
    }

    #[test]
    fn off_it_only_tells_and_restart_downloads_first() {
        let mut u = Updates::new(false);
        u.begin_check();
        assert_eq!(u.checked(found("0.3.0")), Step::Nothing, "nothing downloaded on its own");
        assert_eq!(u.status(), available("0.3.0"), "told as soon as it is found");
        assert_eq!(u.restart(), Ok(Step::Download("0.3.0".into())));
        assert_eq!(u.downloaded("0.3.0", true), Step::Install);
    }

    #[test]
    fn off_a_restart_during_a_call_downloads_then_waits_for_the_end() {
        let mut u = Updates::new(false);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.call(true);
        assert_eq!(u.restart(), Ok(Step::Download("0.3.0".into())));
        assert_eq!(u.downloaded("0.3.0", true), Step::Nothing);
        assert_eq!(u.call(false), Step::Install);
    }

    #[test]
    fn off_a_failed_download_drops_the_restart() {
        let mut u = Updates::new(false);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.restart().unwrap();
        assert_eq!(u.downloaded("0.3.0", false), Step::Nothing);
        assert_eq!(u.status(), available("0.3.0"), "still there to ask again");
        assert_eq!(u.restart(), Ok(Step::Download("0.3.0".into())), "asked again, tried again");
    }

    #[test]
    fn turning_it_on_downloads_what_was_found() {
        let mut u = Updates::new(false);
        u.begin_check();
        u.checked(found("0.3.0"));
        assert_eq!(u.set_auto_download(true), Step::Download("0.3.0".into()));
        assert_eq!(u.status(), Status::default(), "on: told once downloaded");
        u.downloaded("0.3.0", true);
        assert_eq!(u.set_auto_download(false), Step::Nothing);
        assert_eq!(u.status(), available("0.3.0"));
        assert_eq!(u.restart(), Ok(Step::Install), "already downloaded: no second download");
    }

    #[test]
    fn a_failed_install_offers_nothing_until_the_next_check() {
        let mut u = Updates::new(true);
        u.begin_check();
        u.checked(found("0.3.0"));
        u.downloaded("0.3.0", true);
        assert_eq!(u.restart(), Ok(Step::Install));
        u.install_failed();
        assert_eq!(u.status(), Status::default());
        assert_eq!(u.call(false), Step::Nothing);
        assert!(u.begin_check());
        assert_eq!(u.checked(found("0.3.0")), Step::Download("0.3.0".into()));
    }
}
