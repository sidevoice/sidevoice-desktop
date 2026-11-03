//! The developer options of the phone (`window.__sidevoiceHost.host.debug`, docs/BRIDGE.md → "The developer options"): in a
//! nightly or a dev build, the switches the room's hidden "Developer options" section turns. Off by default, kept in a small
//! file of the app's own files, and absent (every switch off) in a release.
//!
//! One switch so far: `systemCall` ("Treat calls as system calls", D-NO-CALLKIT-DEFAULT). Off, the room's call is the app's
//! own: no CallKit provider is made on iOS, and Android's call service registers nothing with Telecom (docs/MOBILE.md → "The
//! system's call"). On, the call is a system's call (CallKit, Core-Telecom), for testing with a headset or in a car.
use std::{
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

/// The switches this phone has, in the order the page lists them (`host.debug.available`).
pub const AVAILABLE: [&str; 1] = ["systemCall"];

/// The record's file name, in the app's own files.
const FILE: &str = "developer-options.json";

/// Whether a build of `channel` offers the developer options: a nightly or a dev build, never a release (the same rule as
/// the desktop's `debug_options::offered`).
pub fn offered(channel: &str) -> bool {
    channel != "release"
}

/// What is kept: every switch absent from the record is off.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Kept {
    system_call: bool,
}

/// The app's developer options: what the page changed, read by the call.
pub struct DeveloperOptions {
    /// Where they are kept; `None` in a build that does not offer them.
    file: Option<PathBuf>,
    system_call: AtomicBool,
}

impl DeveloperOptions {
    /// The options of the app's build `channel`, read from its own files: a record that is missing or unreadable is every
    /// switch off, and a release has none whatever a record says.
    pub fn of_app<R: Runtime>(app: &AppHandle<R>, channel: &str) -> tauri::Result<Self> {
        Ok(Self::new(&crate::own_files(app)?, channel))
    }

    fn new(files: &std::path::Path, channel: &str) -> Self {
        if !offered(channel) {
            return Self {
                file: None,
                system_call: AtomicBool::new(false),
            };
        }
        let file = files.join(FILE);
        let kept = fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok())
            .unwrap_or_default();
        Self {
            file: Some(file),
            system_call: AtomicBool::new(kept.system_call),
        }
    }

    /// Whether the room's call is a system's call (CallKit, Telecom). Off unless the person turned it on.
    pub fn system_call(&self) -> bool {
        self.system_call.load(Ordering::Relaxed)
    }

    /// The switches as the page reads them (`host.debug`'s state), or `None` where they are not offered.
    pub fn state(&self) -> Option<Value> {
        self.file
            .as_ref()
            .map(|_| json!({ "available": AVAILABLE, "systemCall": self.system_call() }))
    }

    /// Turns the switch `name` on or off, and keeps it. Refused for a switch this phone lacks and in a build without the
    /// options; the new state is what the page reads back.
    pub fn set(&self, name: &str, on: bool) -> Result<Value, String> {
        let file = self
            .file
            .as_ref()
            .ok_or("The developer options are not in this build.")?;
        match name {
            "systemCall" => {
                let kept = Kept { system_call: on };
                if let Some(directory) = file.parent() {
                    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
                }
                write(file, &kept).map_err(|error| error.to_string())?;
                self.system_call.store(on, Ordering::Relaxed);
            }
            _ => return Err(format!("not a developer option: {name}")),
        }
        Ok(self.state().unwrap_or(Value::Null))
    }
}

/// The record written whole to a temporary file beside it and renamed over the old one, so that a crash leaves the old
/// record or the new one.
fn write(file: &std::path::Path, kept: &Kept) -> io::Result<()> {
    let temporary = file.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec(kept)?)?;
    fs::rename(&temporary, file)
}

/// Whether the room's call is a system's call now: the app's developer option, off when it is not managed (a test) or not
/// offered.
pub fn system_call<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<DeveloperOptions>()
        .is_some_and(|options| options.system_call())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_offers_none_and_a_nightly_or_a_dev_build_offers_them() {
        assert!(!offered("release"));
        assert!(offered("nightly"));
        assert!(offered("dev"));
    }

    #[test]
    fn the_call_is_the_app_s_own_until_the_person_turns_it_on() {
        let files = tempfile::tempdir().unwrap();
        let options = DeveloperOptions::new(files.path(), "nightly");
        assert!(!options.system_call());
        assert_eq!(
            options.state(),
            Some(json!({"available": ["systemCall"], "systemCall": false}))
        );
    }

    #[test]
    fn a_switch_is_kept_across_a_restart_and_turned_off_again() {
        let files = tempfile::tempdir().unwrap();
        let options = DeveloperOptions::new(files.path(), "dev");
        assert_eq!(
            options.set("systemCall", true),
            Ok(json!({"available": ["systemCall"], "systemCall": true}))
        );
        assert!(options.system_call());
        assert!(DeveloperOptions::new(files.path(), "dev").system_call());
        options.set("systemCall", false).unwrap();
        assert!(!DeveloperOptions::new(files.path(), "dev").system_call());
    }

    #[test]
    fn a_release_ignores_a_kept_switch_and_refuses_to_change_one() {
        let files = tempfile::tempdir().unwrap();
        DeveloperOptions::new(files.path(), "nightly")
            .set("systemCall", true)
            .unwrap();
        let release = DeveloperOptions::new(files.path(), "release");
        assert!(!release.system_call());
        assert_eq!(release.state(), None);
        assert!(release.set("systemCall", true).is_err());
        assert!(!release.system_call());
    }

    #[test]
    fn what_is_not_a_switch_is_refused_and_an_unreadable_record_is_all_off() {
        let files = tempfile::tempdir().unwrap();
        let options = DeveloperOptions::new(files.path(), "nightly");
        assert!(options.set("inspectable", true).is_err());
        fs::write(files.path().join(FILE), b"not json").unwrap();
        assert!(!DeveloperOptions::new(files.path(), "nightly").system_call());
    }
}
