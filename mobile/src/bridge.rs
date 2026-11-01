//! The page's script (docs/MOBILE.md -> "The bridge"): `window.__sidevoiceHost`, made of the three parts every app
//! composes (docs/BRIDGE.md -> "Parts"): the shell binding's transport (`sidevoice_app_tauri::js::TRANSPORT`), the members
//! every app shares (`sidevoice_app_core::js::MEMBERS`) and this app's own part, `js/mobile-bridge.js`, which assembles
//! `host` from them.

/// The mobile app's part of the script, with its placeholders still in place.
const SCRIPT_TEMPLATE: &str = include_str!("../js/mobile-bridge.js");
const ORIGIN_PLACEHOLDER: &str = "\"__SIDEVOICE_APP_ORIGIN__\"";
const MICROPHONE_SETTINGS_PLACEHOLDER: &str = "\"__SIDEVOICE_MICROPHONE_SETTINGS__\"";

/// The script injected into the app's window, bound to the app's own `origin`: on any other it does nothing.
/// `microphone_settings`: whether the system has a settings page for the app's microphone permission
/// (`host.microphone.openSettings`).
pub fn script(origin: &str, microphone_settings: bool) -> String {
    let literal = serde_json::to_string(origin).expect("a string serialises");
    let mobile = SCRIPT_TEMPLATE
        .replacen(ORIGIN_PLACEHOLDER, &literal, 1)
        .replacen(
            MICROPHONE_SETTINGS_PLACEHOLDER,
            if microphone_settings { "true" } else { "false" },
            1,
        );
    compose(
        sidevoice_app_tauri::js::TRANSPORT,
        sidevoice_app_core::js::MEMBERS,
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
        for placeholder in [ORIGIN_PLACEHOLDER, MICROPHONE_SETTINGS_PLACEHOLDER] {
            assert_eq!(
                SCRIPT_TEMPLATE.matches(placeholder).count(),
                1,
                "{placeholder}"
            );
        }
        let ios = script("tauri://localhost", true);
        for placeholder in [ORIGIN_PLACEHOLDER, MICROPHONE_SETTINGS_PLACEHOLDER] {
            assert!(!ios.contains(placeholder), "{placeholder} is left");
        }
        assert!(ios.contains("factory(window, \"tauri://localhost\", true)"));
        assert!(script("http://tauri.localhost", false)
            .contains("factory(window, \"http://tauri.localhost\", false)"));
    }

    #[test]
    fn the_script_is_made_of_the_real_parts() {
        let composed = script("tauri://localhost", true);
        assert!(composed.starts_with("(function (transportOf, membersOf) {\n"));
        assert!(composed.contains(sidevoice_app_tauri::js::TRANSPORT));
        assert!(composed.contains(sidevoice_app_core::js::MEMBERS));
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
}
