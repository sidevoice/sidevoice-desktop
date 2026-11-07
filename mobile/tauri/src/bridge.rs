//! The page's script (docs/MOBILE.md -> "The bridge"): `window.__sidevoiceHost`, made of the three parts every app
//! composes (docs/BRIDGE.md -> "Parts"): the shell binding's transport (`sidevoice_shared_tauri::js::TRANSPORT`), the members
//! every app shares (`sidevoice_shared_tauri::js::MEMBERS`) and this app's own part, `js/mobile-bridge.js`, which assembles
//! `host` from them.

/// The mobile app's part of the script, with its placeholders still in place.
const SCRIPT_TEMPLATE: &str = include_str!("../js/mobile-bridge.js");
const ORIGIN_PLACEHOLDER: &str = "\"__SIDEVOICE_APP_ORIGIN__\"";
const MICROPHONE_SETTINGS_PLACEHOLDER: &str = "\"__SIDEVOICE_MICROPHONE_SETTINGS__\"";
const MICROPHONE_HELP_PLACEHOLDER: &str = "\"__SIDEVOICE_MICROPHONE_HELP__\"";
const MESSAGES_PLACEHOLDER: &str = "\"__SIDEVOICE_MESSAGES__\"";
const DEVICE_NAME_PLACEHOLDER: &str = "\"__SIDEVOICE_DEVICE_NAME__\"";
const BUILD_PLACEHOLDER: &str = "\"__SIDEVOICE_BUILD__\"";
const DEBUG_PLACEHOLDER: &str = "\"__SIDEVOICE_DEBUG__\"";

/// The app's wording of the web's texts that a phone changes, in every language (`host.messages(language)`; sidevoice-shared's
/// members): `{ "<language>": { "<web catalogue key>": text } }`, this OS's tables over what every app shares.
pub fn messages() -> serde_json::Value {
    sidevoice_shared_tauri::messages::tables(crate::platform::message_overlay::texts())
}

/// How to allow a blocked microphone on this phone, in every language the app words it in
/// (`host.microphone.help(language)`; sidevoice-shared's members): `{ "<language>": { steps: [text], openSettings?:
/// label } }`, the label only where the system has the settings page. `None` where the app words no help (the page then
/// says nothing of its own system).
pub fn microphone_help() -> Option<serde_json::Value> {
    use crate::platform::microphone_help;
    let help = microphone_help::help()?;
    let label = microphone_help::settings_label()
        .filter(|_| sidevoice_shared_tauri::platform::microphone::settings_available());
    let entry = |language: &str| {
        let mut entry = serde_json::json!({ "steps": [crate::i18n::t(language, help)] });
        if let Some(label) = label {
            entry["openSettings"] = crate::i18n::t(language, label).into();
        }
        entry
    };
    Some(
        crate::i18n::LANGUAGES
            .iter()
            .map(|&language| (language.to_owned(), entry(language)))
            .collect::<serde_json::Map<_, _>>()
            .into(),
    )
}

/// The name the person gave this phone (`host.deviceName`, docs/BRIDGE.md), if the system has one.
pub fn device_name() -> Option<String> {
    crate::platform::device_name::device_name()
}

/// What this build is (`host.build`, docs/BRIDGE.md -> "The build"), in the desktop's shape: `{channel, version, commit?,
/// date?}`, the channel `release`, `nightly` or `dev` as `cargo xtask` told the build script (build.rs); an empty commit
/// or date is left out.
pub fn build(channel: &str, version: &str, commit: &str, date: &str) -> serde_json::Value {
    let mut build = serde_json::json!({ "channel": channel, "version": version });
    for (key, value) in [("commit", commit), ("date", date)] {
        if !value.is_empty() {
            build[key] = value.into();
        }
    }
    build
}

/// The script injected into the app's window, bound to the app's own `origin`: on any other it does nothing.
/// `microphone_settings`: whether the system has a settings page for the app's microphone permission
/// (`host.microphone.openSettings`); `microphone_help`: the app's words for a blocked microphone ([`microphone_help`]);
/// `messages`: its wording of the web's texts a phone changes ([`messages`]); `device_name`: the name the person gave the
/// phone ([`device_name`]), none to let the page name it; `build`: what this build is ([`build`]); `debug`: the developer
/// options' state (`host.debug`, [`crate::developer_options::DeveloperOptions::state`]), none in a release (no `host.debug`).
pub fn script(
    origin: &str,
    microphone_settings: bool,
    microphone_help: Option<&serde_json::Value>,
    messages: &serde_json::Value,
    device_name: Option<&str>,
    build: &serde_json::Value,
    debug: Option<&serde_json::Value>,
) -> String {
    let literal = serde_json::to_string(origin).expect("a string serialises");
    let mobile = SCRIPT_TEMPLATE
        .replacen(ORIGIN_PLACEHOLDER, &literal, 1)
        .replacen(
            MICROPHONE_SETTINGS_PLACEHOLDER,
            if microphone_settings { "true" } else { "false" },
            1,
        )
        .replacen(
            MICROPHONE_HELP_PLACEHOLDER,
            &microphone_help.map_or_else(|| "null".to_owned(), |help| help.to_string()),
            1,
        )
        .replacen(MESSAGES_PLACEHOLDER, &messages.to_string(), 1)
        .replacen(
            DEVICE_NAME_PLACEHOLDER,
            &serde_json::to_string(&device_name).expect("a name serialises"),
            1,
        )
        .replacen(BUILD_PLACEHOLDER, &build.to_string(), 1)
        .replacen(
            DEBUG_PLACEHOLDER,
            &debug.map_or_else(|| "null".to_owned(), |debug| debug.to_string()),
            1,
        );
    compose(
        sidevoice_shared_tauri::js::TRANSPORT,
        sidevoice_shared_tauri::js::MEMBERS,
        &mobile,
    )
}

/// The app's part, in a scope where the other two are `transportOf` and `membersOf` (as the desktop composes its own,
/// desktop/core/src/bridge.rs).
fn compose(transport: &str, members: &str, mobile: &str) -> String {
    format!("(function (transportOf, membersOf) {{\n{mobile}\n}})({transport}, {members});")
}

/// What a headset's presses do on the phone (`host.shortcuts`, docs/BRIDGE.md → "The shortcuts"): the single press (play/pause,
/// the AirPods stem) mutes by default and the double press (the next-track command) does nothing by default; the person chooses
/// in Settings › Shortcuts, which shows these rows and nothing else on a phone (no global keys, no keys of the page's own), and
/// only what reaches the app here (`media_controls::presses`: on iOS the single press is the system's mute, with no double press).
/// Kept in a small file of the app's own files, as the developer options are. The press itself reaches the app through
/// `sidevoice-shared-tauri`'s media controls (Now Playing on iOS, the call's media session on Android); what each press does,
/// and the status the page reads, are `sidevoice-shared-headset`'s.
pub mod headset {
    use std::{
        fs, io,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    use serde_json::{json, Value};
    use sidevoice_shared_headset::{
        Headset, HeadsetCapabilities, HeadsetChange, HeadsetChoice, HeadsetPress, Presses,
    };
    use sidevoice_shared_tauri::platform::media_controls;
    use tauri::{AppHandle, Manager, Runtime};

    /// The record's file name, in the app's own files.
    const FILE: &str = "headset.json";

    /// The choice the person made, kept across launches, and what the presses can do here.
    pub struct HeadsetSettings {
        file: PathBuf,
        /// Kept as the person made it, whatever the presses can do here: only their own change changes it.
        choice: Mutex<HeadsetChoice>,
        capabilities: HeadsetCapabilities,
        /// Who owns the call being held, while one is: `Some(true)` the system's (CallKit, Telecom), `Some(false)` the app's own,
        /// whatever the developer option says now (the option applies to the next call; a call the system refused is the app's).
        owner: Mutex<Option<bool>>,
    }

    impl HeadsetSettings {
        /// The choice kept in the app's own files; a record that is missing or unreadable is the defaults.
        pub fn of_app<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Self> {
            Ok(Self::new(
                &crate::own_files(app)?,
                media_controls::presses(),
            ))
        }

        fn new(files: &Path, capabilities: HeadsetCapabilities) -> Self {
            let file = files.join(FILE);
            let choice = fs::read(&file)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<HeadsetChoice>(&bytes).ok())
                .unwrap_or_default();
            Self {
                file,
                choice: Mutex::new(choice),
                capabilities,
                owner: Mutex::new(None),
            }
        }

        pub fn choice(&self) -> HeadsetChoice {
            *self.choice.lock().unwrap()
        }

        /// The room says who owns its call now: `Some(system)` while it holds one, `None` when there is none.
        pub fn call_owner(&self, owner: Option<bool>) {
            *self.owner.lock().unwrap() = owner;
        }

        /// Whether the system's call holds the headset: the owner of the call in progress, else (between calls) what the next call
        /// will be, the developer option `option`.
        pub fn managed(&self, option: bool) -> bool {
            self.owner.lock().unwrap().unwrap_or(option)
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

        /// `host.shortcuts.get()` on a phone: only the headset group, with what each press can do here (`actions`; no double press
        /// where none reaches the app). No global shortcut is offered (`shortcuts` is empty), none needs a permission, and the page
        /// has no keys of its own to offer (`inApp: false`). While the system's call holds the headset (`option`, see
        /// [`Self::managed`]), the rows say so.
        pub fn status(&self, option: bool) -> Value {
            json!({
                "shortcuts": {},
                "permission": null,
                "inApp": false,
                "headset": Headset::of(self.choice(), self.managed(option), self.capabilities),
            })
        }

        /// `host.shortcuts.setHeadset(change)`: keeps it and answers the status. Refused while the system's call holds the
        /// headset (the system mutes; the choice is not the app's to change then), and for what a press cannot do here.
        pub fn set(&self, headset: Option<&Value>, option: bool) -> Result<Value, String> {
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
            if self.managed(option) && !change.is_empty() {
                return Err("the system's call holds the headset's buttons".into());
            }
            self.capabilities.allow(change)?;
            self.change(change).map_err(|error| error.to_string())?;
            Ok(self.status(option))
        }

        /// The command of the room a press of the headset asks for (`run({command})`), or none. The single press is the mute of
        /// the native path (`call::RoomCall::toggle_mute`) when it mutes.
        pub fn command(&self, which: Presses, option: bool) -> Option<&'static str> {
            self.choice()
                .for_press(which, self.managed(option), self.capabilities)
                .command()
        }

        /// Whether the double press has an action: Android's media session advertises skip-to-next only then
        /// (`CallState::double_press_wanted`).
        pub fn double_press_wanted(&self, option: bool) -> bool {
            self.choice()
                .for_press(Presses::Double, self.managed(option), self.capabilities)
                != HeadsetPress::None
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

        const EVERY: HeadsetCapabilities = HeadsetCapabilities::EVERY;
        const SYSTEM_MUTE: HeadsetCapabilities = HeadsetCapabilities::SYSTEM_MUTE;

        /// A phone whose presses can do everything (Android).
        fn settings() -> (tempfile::TempDir, HeadsetSettings) {
            let files = tempfile::tempdir().unwrap();
            let settings = HeadsetSettings::new(files.path(), EVERY);
            (files, settings)
        }

        #[test]
        fn a_phone_reports_only_the_headset_and_mutes_on_one_press_by_default() {
            let (_files, settings) = settings();
            let every = ["mute", "oneTurn", "skip", "none"];
            assert_eq!(
                settings.status(false),
                json!({
                    "shortcuts": {},
                    "permission": null,
                    "inApp": false,
                    "headset": {
                        "press": "mute",
                        "doublePress": "none",
                        "outsideCall": "leave",
                        "actions": {"press": every, "doublePress": every},
                    },
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
            let again = HeadsetSettings::new(files.path(), EVERY);
            assert_eq!(again.command(Presses::Single, false), Some("skip-reply"));
            assert_eq!(again.command(Presses::Double, false), Some("one-turn"));
            assert!(again.double_press_wanted(false));
        }

        /// desktop#464: the double press set, then another setting changed, and it read Nothing. Nothing but the person's own
        /// change changes the choice: not reading the status again, not a call's owner coming and going, not the developer option,
        /// not a change of the other press, not a restart.
        #[test]
        fn the_choice_survives_everything_but_the_person_s_own_change() {
            let (files, settings) = settings();
            settings
                .set(Some(&json!({"doublePress": "oneTurn"})), false)
                .unwrap();
            let double = |settings: &HeadsetSettings, option| {
                settings.status(option)["headset"]["doublePress"].clone()
            };
            for _ in 0..3 {
                assert_eq!(double(&settings, false), "oneTurn", "the status read again");
            }
            assert_eq!(
                double(&settings, true),
                "oneTurn",
                "the developer option on: shown as kept"
            );
            settings.call_owner(Some(true));
            settings.call_owner(Some(false));
            settings.call_owner(None);
            assert_eq!(
                double(&settings, false),
                "oneTurn",
                "a call's owner came and went"
            );
            settings.set(Some(&json!({})), false).unwrap();
            settings
                .set(Some(&json!({"doublePress": null})), false)
                .unwrap();
            settings
                .set(Some(&json!({"press": "skip"})), false)
                .unwrap();
            assert_eq!(
                double(&settings, false),
                "oneTurn",
                "a change of nothing, or of the other press"
            );
            assert!(
                settings
                    .set(Some(&json!({"doublePress": "none"})), true)
                    .is_err(),
                "refused while the system holds the headset"
            );
            let restarted = HeadsetSettings::new(files.path(), EVERY);
            assert_eq!(double(&restarted, false), "oneTurn", "a restart");
            restarted
                .set(Some(&json!({"doublePress": "none"})), false)
                .unwrap();
            assert_eq!(double(&restarted, false), "none", "the person's own change");
            assert_eq!(
                double(&HeadsetSettings::new(files.path(), EVERY), false),
                "none"
            );
        }

        #[test]
        fn where_only_the_system_s_mute_reaches_the_app_the_single_press_mutes_and_there_is_no_double_press(
        ) {
            let files = tempfile::tempdir().unwrap();
            // A choice kept by a build whose presses could do more (TestFlight 61 on iOS).
            fs::write(
                files.path().join(FILE),
                br#"{"press":"skip","doublePress":"oneTurn"}"#,
            )
            .unwrap();
            let settings = HeadsetSettings::new(files.path(), SYSTEM_MUTE);
            assert_eq!(
                settings.status(false)["headset"],
                json!({"press": "mute", "doublePress": null, "outsideCall": "leave", "actions": {"press": ["mute"]}})
            );
            assert_eq!(
                settings.command(Presses::Single, false),
                Some("toggle-mute")
            );
            assert_eq!(settings.command(Presses::Double, false), None);
            assert!(!settings.double_press_wanted(false));
            assert!(settings
                .set(Some(&json!({"press": "skip"})), false)
                .is_err());
            assert!(settings
                .set(Some(&json!({"doublePress": "oneTurn"})), false)
                .is_err());
            assert!(settings.set(Some(&json!({"press": "mute"})), false).is_ok());
            assert_eq!(
                HeadsetSettings::new(files.path(), EVERY)
                    .choice()
                    .double_press,
                HeadsetPress::OneTurn,
                "what is kept is the person's, untouched"
            );
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
                HeadsetSettings::new(files.path(), EVERY).choice(),
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

        #[test]
        fn the_owner_of_the_call_in_progress_decides_not_the_option() {
            let (_files, settings) = settings();
            settings
                .set(Some(&json!({"press": "skip"})), false)
                .unwrap();
            // The option is on, but the system refused the call and it went on as the app's own: the rows are the app's.
            settings.call_owner(Some(false));
            assert!(!settings.managed(true));
            assert!(settings.status(true)["headset"]
                .get("managedBySystem")
                .is_none());
            assert_eq!(settings.command(Presses::Single, true), Some("skip-reply"));
            assert!(settings.set(Some(&json!({"press": "mute"})), true).is_ok());
            // The option is turned off in a system's call: it applies to the next call, this one stays the system's.
            settings.call_owner(Some(true));
            assert!(settings.managed(false));
            assert_eq!(
                settings.command(Presses::Single, false),
                Some("toggle-mute")
            );
            assert!(settings
                .set(Some(&json!({"press": "skip"})), false)
                .is_err());
            // Between calls, the option says what the next one will be.
            settings.call_owner(None);
            assert!(settings.managed(true));
            assert!(!settings.managed(false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> serde_json::Value {
        build("dev", "0.1.0", "", "")
    }

    #[test]
    fn every_placeholder_is_filled_once() {
        for placeholder in [
            ORIGIN_PLACEHOLDER,
            MICROPHONE_SETTINGS_PLACEHOLDER,
            MICROPHONE_HELP_PLACEHOLDER,
            MESSAGES_PLACEHOLDER,
            DEVICE_NAME_PLACEHOLDER,
            BUILD_PLACEHOLDER,
            DEBUG_PLACEHOLDER,
        ] {
            assert_eq!(
                SCRIPT_TEMPLATE.matches(placeholder).count(),
                1,
                "{placeholder}"
            );
        }
        let ios = script(
            "tauri://localhost",
            true,
            None,
            &messages(),
            None,
            &dev(),
            None,
        );
        for placeholder in [
            ORIGIN_PLACEHOLDER,
            MICROPHONE_SETTINGS_PLACEHOLDER,
            MICROPHONE_HELP_PLACEHOLDER,
            MESSAGES_PLACEHOLDER,
            DEVICE_NAME_PLACEHOLDER,
            BUILD_PLACEHOLDER,
            DEBUG_PLACEHOLDER,
        ] {
            assert!(!ios.contains(placeholder), "{placeholder} is left");
        }
        assert!(ios.contains(&format!(
            "factory(window, \"tauri://localhost\", true, null, {}, null, {}, null)",
            messages(),
            dev()
        )));
        assert!(script(
            "http://tauri.localhost",
            false,
            None,
            &messages(),
            None,
            &dev(),
            None
        )
        .contains(&format!(
            "factory(window, \"http://tauri.localhost\", false, null, {}, null, {}, null)",
            messages(),
            dev()
        )));
    }

    #[test]
    fn the_phone_s_wording_is_in_every_language_over_the_shared_one_and_says_where_the_speaker_is_chosen(
    ) {
        assert_eq!(
            sidevoice_shared_tauri::messages::check(crate::platform::message_overlay::texts()),
            Ok(())
        );
        let all = messages();
        for language in sidevoice_shared_tauri::messages::LANGUAGES {
            let texts = &all[language];
            for key in [
                "devices.speakerFixed",
                "settings.call.keepAwakeUnsupported",
                "voice.error.browser",
            ] {
                assert!(
                    texts[key].as_str().is_some_and(|text| !text.is_empty()),
                    "{language}: {key}"
                );
            }
            // Nothing a phone says is about a browser.
            assert!(!texts["devices.speakerFixed"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .contains("browser"));
        }
    }

    #[test]
    fn the_device_name_is_given_to_the_page_as_a_string_literal_or_not_at_all() {
        let named = script(
            "tauri://localhost",
            true,
            None,
            &messages(),
            Some("Ana's \"S24\""),
            &dev(),
            None,
        );
        assert!(named.contains(&format!(
            "factory(window, \"tauri://localhost\", true, null, {}, \"Ana's \\\"S24\\\"\", {}, null)",
            messages(),
            dev()
        )));
    }

    #[test]
    fn the_build_says_its_channel_and_only_what_it_has() {
        assert_eq!(
            dev(),
            serde_json::json!({"channel": "dev", "version": "0.1.0"})
        );
        assert_eq!(
            build("nightly", "0.1.0", "a1b2c3d", "2026-11-02"),
            serde_json::json!({"channel": "nightly", "version": "0.1.0", "commit": "a1b2c3d", "date": "2026-11-02"})
        );
    }

    #[test]
    fn the_developer_options_are_given_to_the_page_as_json_or_not_at_all() {
        let state = serde_json::json!({"available": ["systemCall"], "systemCall": false});
        let offered = script(
            "tauri://localhost",
            true,
            None,
            &messages(),
            None,
            &dev(),
            Some(&state),
        );
        assert!(offered.contains(&format!(
            "factory(window, \"tauri://localhost\", true, null, {}, null, {}, {state})",
            messages(),
            dev()
        )));
    }

    #[test]
    fn the_script_is_made_of_the_real_parts() {
        let composed = script(
            "tauri://localhost",
            true,
            None,
            &messages(),
            None,
            &dev(),
            None,
        );
        assert!(composed.starts_with("(function (transportOf, membersOf) {\n"));
        assert!(composed.contains(sidevoice_shared_tauri::js::TRANSPORT));
        assert!(composed.contains(sidevoice_shared_tauri::js::MEMBERS));
    }

    #[test]
    fn the_page_is_told_what_build_it_is() {
        assert!(SCRIPT_TEMPLATE.contains("build: Object.freeze({ ...build })"));
    }

    #[test]
    fn the_page_is_told_the_app_owns_the_media_and_nothing_of_the_desktop_s() {
        assert!(SCRIPT_TEMPLATE.contains("presence: Object.freeze({ media: \"app\" })"));
        // The members of `host` the desktop has, as the host object would name them.
        for desktop in [
            "localHost:",
            "reset:",
            "muteShortcut:",
            "callControls:",
            "updater:",
            "os:",
            "build:",
        ] {
            assert!(!SCRIPT_TEMPLATE.contains(desktop), "{desktop}");
        }
    }

    #[test]
    fn the_microphone_help_is_the_app_s_own_words_in_every_language_with_the_button_where_there_is_a_page(
    ) {
        let Some(help) = microphone_help() else {
            assert!(crate::platform::microphone_help::help().is_none());
            return;
        };
        for language in crate::i18n::LANGUAGES {
            let entry = &help[language];
            assert!(
                entry["steps"][0]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{language}"
            );
            assert_eq!(
                entry.get("openSettings").is_some(),
                sidevoice_shared_tauri::platform::microphone::settings_available()
                    && crate::platform::microphone_help::settings_label().is_some(),
                "{language}"
            );
        }
        let script = script(
            "tauri://localhost",
            true,
            Some(&help),
            &messages(),
            None,
            &dev(),
            None,
        );
        assert!(script.contains(&format!(
            "factory(window, \"tauri://localhost\", true, {help}, {}, null, {}, null)",
            messages(),
            dev()
        )));
    }
}
