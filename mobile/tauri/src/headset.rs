//! What a headset's presses do on the phone (`host.shortcuts`, docs/BRIDGE.md → "The shortcuts"): the single press (play/pause,
//! the AirPods stem) mutes by default and the double press (the next-track command) does nothing by default; the person chooses
//! in Settings › Shortcuts, which shows these two rows and nothing else on a phone (no global keys, no keys of the page's own).
//! Kept in a small file of the app's own files, as the developer options are. The press itself reaches the app through
//! `sidevoice-shared-tauri`'s media controls (Now Playing on iOS, the call's media session on Android); what each press does,
//! and the status the page reads, are `sidevoice-shared-headset`'s.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde_json::{json, Value};
use sidevoice_shared_headset::{Headset, HeadsetChange, HeadsetChoice, HeadsetPress, Presses};
use tauri::{AppHandle, Manager, Runtime};

/// The record's file name, in the app's own files.
const FILE: &str = "headset.json";

/// The choice the person made, kept across launches.
pub struct HeadsetSettings {
    file: PathBuf,
    choice: Mutex<HeadsetChoice>,
}

impl HeadsetSettings {
    /// The choice kept in the app's own files; a record that is missing or unreadable is the defaults.
    pub fn of_app<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Self> {
        Ok(Self::new(&crate::own_files(app)?))
    }

    fn new(files: &Path) -> Self {
        let file = files.join(FILE);
        let choice = fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HeadsetChoice>(&bytes).ok())
            .unwrap_or_default();
        Self {
            file,
            choice: Mutex::new(choice),
        }
    }

    pub fn choice(&self) -> HeadsetChoice {
        *self.choice.lock().unwrap()
    }

    /// Applies the page's `change` and keeps it. Whether anything changed is the caller's to ask of the change.
    fn change(&self, change: HeadsetChange) -> io::Result<()> {
        let mut choice = self.choice.lock().unwrap();
        let next = choice.with(change);
        if next != *choice {
            if let Some(directory) = self.file.parent() {
                fs::create_dir_all(directory)?;
            }
            // Whole to a temporary file beside it, renamed over the old one: a crash leaves the old record or the new one.
            let temporary = self.file.with_extension("json.tmp");
            fs::write(&temporary, serde_json::to_vec(&next)?)?;
            fs::rename(&temporary, &self.file)?;
            *choice = next;
        }
        Ok(())
    }

    /// `host.shortcuts.get()` on a phone: only the headset group. No global shortcut is offered (`shortcuts` is empty), none
    /// needs a permission, and the page has no keys of its own to offer (`inApp: false`). While the system's call holds the
    /// headset (`system_call`), the rows say so.
    pub fn status(&self, system_call: bool) -> Value {
        json!({
            "shortcuts": {},
            "permission": null,
            "inApp": false,
            "headset": Headset::of(self.choice(), system_call),
        })
    }

    /// `host.shortcuts.setHeadset(change)`: keeps it and answers the status. Refused while the system's call holds the
    /// headset (the system mutes; the choice is not the app's to change then).
    pub fn set(&self, headset: Option<&Value>, system_call: bool) -> Result<Value, String> {
        let object = headset
            .and_then(Value::as_object)
            .ok_or("setHeadset(change) takes an object.")?;
        let change = HeadsetChange::parse(
            object.get("press").and_then(Value::as_str),
            object.get("doublePress"),
            object.get("outsideCall").and_then(Value::as_str),
        )?;
        if object.get("press").is_some_and(|value| !value.is_string()) {
            return Err(format!("no such press: {}", object["press"]));
        }
        if system_call && !change.is_empty() {
            return Err("the system's call holds the headset's buttons".into());
        }
        self.change(change).map_err(|error| error.to_string())?;
        Ok(self.status(system_call))
    }

    /// The command of the room a press of the headset asks for (`run({command})`), or none. The single press is the mute of
    /// the native path (`call::RoomCall::toggle_mute`) when it mutes.
    pub fn command(&self, which: Presses, system_call: bool) -> Option<&'static str> {
        self.choice().for_press(which, system_call).command()
    }

    /// Whether the double press has an action: iOS registers the next-track command only then (`CallState::double_press_wanted`).
    pub fn double_press_wanted(&self, system_call: bool) -> bool {
        self.choice().for_press(Presses::Double, system_call) != HeadsetPress::None
    }
}

/// Whether the system's call holds the headset now (the developer option "Treat calls as system calls").
pub fn system_call<R: Runtime>(app: &AppHandle<R>) -> bool {
    crate::developer_options::system_call(app)
}

/// A press of the headset arrived: what the person chose for it. Run on the page's room (`one-turn`, `skip-reply`), or the
/// native mute (the headset shows none; the other of the app's own).
pub fn pressed(app: &AppHandle, which: Presses) {
    let Some(settings) = app.try_state::<HeadsetSettings>() else {
        return;
    };
    match settings.command(which, system_call(app)) {
        Some("toggle-mute") => {
            if let Some(room) = app.try_state::<crate::call::RoomCall>() {
                room.toggle_mute();
            }
        }
        Some(command) => crate::call::run_on_page(app, json!({ "command": command })),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> (tempfile::TempDir, HeadsetSettings) {
        let files = tempfile::tempdir().unwrap();
        let settings = HeadsetSettings::new(files.path());
        (files, settings)
    }

    #[test]
    fn a_phone_reports_only_the_headset_and_mutes_on_one_press_by_default() {
        let (_files, settings) = settings();
        assert_eq!(
            settings.status(false),
            json!({
                "shortcuts": {},
                "permission": null,
                "inApp": false,
                "headset": {"press": "mute", "doublePress": "none", "outsideCall": "leave"},
            })
        );
        assert_eq!(
            settings.command(Presses::Single, false),
            Some("toggle-mute")
        );
        assert_eq!(settings.command(Presses::Double, false), None);
        assert!(!settings.double_press_wanted(false));
    }

    #[test]
    fn a_choice_is_kept_across_a_restart_and_what_is_not_named_stays() {
        let (files, settings) = settings();
        let status = settings
            .set(Some(&json!({"doublePress": "oneTurn"})), false)
            .unwrap();
        assert_eq!(status["headset"]["doublePress"], "oneTurn");
        assert_eq!(status["headset"]["press"], "mute");
        settings
            .set(Some(&json!({"press": "skip"})), false)
            .unwrap();
        let again = HeadsetSettings::new(files.path());
        assert_eq!(again.command(Presses::Single, false), Some("skip-reply"));
        assert_eq!(again.command(Presses::Double, false), Some("one-turn"));
        assert!(again.double_press_wanted(false));
    }

    #[test]
    fn what_is_not_a_press_is_refused_and_an_unreadable_record_is_the_defaults() {
        let (files, settings) = settings();
        assert!(settings
            .set(Some(&json!({"press": "loud"})), false)
            .is_err());
        assert!(settings.set(Some(&json!({"press": 3})), false).is_err());
        assert!(settings
            .set(Some(&json!({"outsideCall": "join"})), false)
            .is_err());
        assert!(settings.set(Some(&json!("mute")), false).is_err());
        assert!(settings.set(None, false).is_err());
        fs::write(files.path().join(FILE), b"not json").unwrap();
        assert_eq!(
            HeadsetSettings::new(files.path()).choice(),
            HeadsetChoice::default()
        );
    }

    #[test]
    fn the_system_s_call_holds_the_headset_the_system_mutes_and_the_choice_waits() {
        let (_files, settings) = settings();
        settings
            .set(
                Some(&json!({"press": "skip", "doublePress": "oneTurn"})),
                false,
            )
            .unwrap();
        let status = settings.status(true);
        assert_eq!(status["headset"]["managedBySystem"], true);
        assert_eq!(
            status["headset"]["press"], "skip",
            "the choice is shown as it is kept"
        );
        assert_eq!(settings.command(Presses::Single, true), Some("toggle-mute"));
        assert_eq!(settings.command(Presses::Double, true), None);
        assert!(!settings.double_press_wanted(true));
        assert!(settings.set(Some(&json!({"press": "mute"})), true).is_err());
        assert_eq!(
            settings.command(Presses::Single, false),
            Some("skip-reply"),
            "kept for when the call is the app's own"
        );
    }
}
