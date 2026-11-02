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

/// The script injected into the app's window, bound to the app's own `origin`: on any other it does nothing.
/// `microphone_settings`: whether the system has a settings page for the app's microphone permission
/// (`host.microphone.openSettings`); `microphone_help`: the app's words for a blocked microphone ([`microphone_help`]);
/// `messages`: its wording of the web's texts a phone changes ([`messages`]); `device_name`: the name the person gave the
/// phone ([`device_name`]), none to let the page name it.
pub fn script(
    origin: &str,
    microphone_settings: bool,
    microphone_help: Option<&serde_json::Value>,
    messages: &serde_json::Value,
    device_name: Option<&str>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_placeholder_is_filled_once() {
        for placeholder in [
            ORIGIN_PLACEHOLDER,
            MICROPHONE_SETTINGS_PLACEHOLDER,
            MICROPHONE_HELP_PLACEHOLDER,
            MESSAGES_PLACEHOLDER,
            DEVICE_NAME_PLACEHOLDER,
        ] {
            assert_eq!(
                SCRIPT_TEMPLATE.matches(placeholder).count(),
                1,
                "{placeholder}"
            );
        }
        let ios = script("tauri://localhost", true, None, &messages(), None);
        for placeholder in [
            ORIGIN_PLACEHOLDER,
            MICROPHONE_SETTINGS_PLACEHOLDER,
            MICROPHONE_HELP_PLACEHOLDER,
            MESSAGES_PLACEHOLDER,
            DEVICE_NAME_PLACEHOLDER,
        ] {
            assert!(!ios.contains(placeholder), "{placeholder} is left");
        }
        assert!(ios.contains(&format!(
            "factory(window, \"tauri://localhost\", true, null, {}, null)",
            messages()
        )));
        assert!(
            script("http://tauri.localhost", false, None, &messages(), None).contains(&format!(
                "factory(window, \"http://tauri.localhost\", false, null, {}, null)",
                messages()
            ))
        );
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
        );
        assert!(named.contains(&format!(
            "factory(window, \"tauri://localhost\", true, null, {}, \"Ana's \\\"S24\\\"\")",
            messages()
        )));
    }

    #[test]
    fn the_script_is_made_of_the_real_parts() {
        let composed = script("tauri://localhost", true, None, &messages(), None);
        assert!(composed.starts_with("(function (transportOf, membersOf) {\n"));
        assert!(composed.contains(sidevoice_shared_tauri::js::TRANSPORT));
        assert!(composed.contains(sidevoice_shared_tauri::js::MEMBERS));
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
            "debug:",
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
        let script = script("tauri://localhost", true, Some(&help), &messages(), None);
        assert!(script.contains(&format!(
            "factory(window, \"tauri://localhost\", true, {help}, {}, null)",
            messages()
        )));
    }
}
