//! What the host computer is and offers: the words and labels its system decides for the page (the shortcut modifier, the
//! microphone's help), its name, and whether this run can update itself. Per-OS differences live in `host/{macos,windows,linux}.rs`.

use sidevoice_desktop_core::{bridge, computer_name, updater};
use tauri::utils::{config::BundleType, platform::bundle_type};

#[cfg_attr(target_os = "macos", path = "host/macos.rs")]
#[cfg_attr(target_os = "windows", path = "host/windows.rs")]
#[cfg_attr(target_os = "linux", path = "host/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// The label of the modifier a shortcut is written with on this system (`host.shortcutModifier`): `⌘` or `Ctrl`.
    const SHORTCUT_MODIFIER: &'static str;

    /// The key, in the native catalogue (core `i18n`), of how to allow a blocked microphone on this system
    /// (`host.microphone.help`).
    const MICROPHONE_HELP: &'static str;

    /// The key of the label on the button that opens the system's microphone settings, named as this system names them;
    /// `None` where the system has no such page to open (`platform::microphone::settings_available`).
    const MICROPHONE_SETTINGS_LABEL: Option<&'static str>;

    /// The name the person gave the device, kept apart from the network name (macOS Sharing's Computer Name,
    /// Linux's pretty hostname); `None` where the OS keeps none.
    fn device_name() -> Option<String> {
        None
    }

    /// The host name, read when there is no device name (Windows' physical DNS name preserving case, Linux's
    /// kernel hostname).
    fn host_name() -> Option<String> {
        None
    }

    /// The bundle the updater plugin can update and restart here; `None` where there is none.
    const BUNDLE: Option<BundleType> = None;

    /// Whether this run is that bundle's installation, not a copy of it.
    fn installed_here() -> bool {
        false
    }
}

/// The label of the modifier a shortcut is written with here (`host.shortcutModifier`, docs/BRIDGE.md).
pub fn shortcut_modifier() -> &'static str {
    Native::SHORTCUT_MODIFIER
}

/// How to allow a blocked microphone here, in every language the app has words in (`host.microphone.help`, docs/BRIDGE.md):
/// the page asks in its own and gets that one, or English.
pub fn microphone_help() -> serde_json::Value {
    bridge::microphone_help(Native::MICROPHONE_HELP, Native::MICROPHONE_SETTINGS_LABEL)
}

/// The name the person gave this computer (`host.deviceName`, docs/BRIDGE.md), not an account or network name.
/// Read on every room open so a rename appears on the next one: the device name, else the host name. Trim
/// surrounding whitespace; return None for absent/blank names so the page can name the device.
pub fn name() -> Option<String> {
    Native::device_name().or_else(Native::host_name).and_then(|name| computer_name::given(&name))
}

/// Whether this run is an installation the updater plugin can update and restart: a release/nightly running
/// from a writable macOS app volume, an AppImage launched as such, or the registered per-user NSIS installation.
/// Dev builds, deb/rpm, MSI, bare binaries and hosts without an update path report `false`.
/// Feed, download, install and restart remain owned by the updater plugin; Tauri's bundle identity decides on every
/// OS, and each OS adds its own proof that this run is that installation.
pub fn updates_available(channel: &str) -> bool {
    updater::feed(channel).is_some()
        && Native::BUNDLE.is_some_and(|bundle| bundle_type() == Some(bundle))
        && Native::installed_here()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_core::i18n;

    #[test]
    fn the_shortcut_modifier_is_a_label_the_page_can_write() {
        assert!(["⌘", "Ctrl"].contains(&shortcut_modifier()));
    }

    #[test]
    fn the_microphone_help_is_worded_in_every_language_the_app_has() {
        let help = microphone_help();
        for language in i18n::LANGUAGES {
            let steps = help[language]["steps"].as_array().unwrap_or_else(|| panic!("{language}: no steps"));
            assert!(!steps.is_empty(), "{language}");
            assert!(
                steps
                    .iter()
                    .all(|step| step.as_str().is_some_and(|text| !text.is_empty() && !text.starts_with("microphone."))),
                "{language}"
            );
        }
    }

    #[test]
    fn the_settings_button_is_named_where_the_system_has_a_page_to_open() {
        assert_eq!(
            Native::MICROPHONE_SETTINGS_LABEL.is_some(),
            sidevoice_app_tauri::platform::microphone::settings_available()
        );
        let help = microphone_help();
        for language in i18n::LANGUAGES {
            assert_eq!(
                help[language].get("openSettings").is_some(),
                Native::MICROPHONE_SETTINGS_LABEL.is_some(),
                "{language}"
            );
        }
    }

    #[test]
    fn a_system_name_is_nonblank_and_preserves_its_given_spelling() {
        if let Some(name) = name() {
            assert!(!name.is_empty());
            assert_eq!(name, name.trim());
            assert_eq!(computer_name::given(&name).as_deref(), Some(name.as_str()));
        }
    }

    #[test]
    fn dev_and_unknown_channels_never_offer_updates() {
        for channel in ["dev", "", "other"] {
            assert!(!updates_available(channel));
        }
    }

    #[test]
    fn release_and_nightly_share_the_installation_requirement() {
        assert_eq!(updates_available("release"), updates_available("nightly"));
        // These tests run as an ordinary test executable, outside any supported updater bundle.
        assert!(!updates_available("release"));
    }
}
