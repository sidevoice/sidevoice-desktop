//! Global shortcut registration and the host's shortcut labels / accessibility reservation.

use sidevoice_desktop_core::settings::{self, MuteShortcut, Registration};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcut, Shortcut, ShortcutState};

#[cfg_attr(target_os = "macos", path = "shortcut_access/macos.rs")]
#[cfg_attr(target_os = "windows", path = "shortcut_access/windows.rs")]
#[cfg_attr(target_os = "linux", path = "shortcut_access/linux.rs")]
mod imp;
use imp::Os;

#[derive(PartialEq, Eq)]
enum LabelStyle {
    Symbols,
    Names,
}

/// The only parts of shortcut handling that differ per OS; everything else is the plain functions below.
trait Native {
    /// Whether this session can use the plugin backend. Linux requires an actual X11 GDK display;
    /// Wayland has no backend in global-hotkey, even when an XWayland DISPLAY is present.
    fn backend_available() -> bool {
        true
    }

    /// Whether the host currently reports its accessibility reservation for this chord. Only macOS observes
    /// VoiceOver; false elsewhere means no implemented observation, not that a chord is safe from every conflict.
    fn reserved_chord_active(_chord: &str) -> bool {
        false
    }

    /// macOS uses modifier symbols; Windows/Linux use readable modifier names.
    fn label_style() -> LabelStyle {
        LabelStyle::Names
    }
}

/// Normalize a kept accelerator and accept it only if the plugin recognizes the key. Empty disables.
pub fn checked(text: &str) -> Option<String> {
    settings::normalize_shortcut(text).filter(|a| a.is_empty() || a.parse::<Shortcut>().is_ok())
}

/// Install the maintained global-shortcut plugin on the UI thread. A missing backend must not stop startup.
pub fn configure(app: &AppHandle) {
    if !Os::backend_available() {
        log::info!("global shortcuts unavailable in this desktop session");
        return;
    }
    let plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                crate::send(app, sidevoice_desktop_core::bridge::Command::ToggleMute);
            }
        })
        .build();
    if let Err(error) = app.plugin(plugin) {
        log::warn!("global shortcuts unavailable: {error}");
    }
}

/// Replace the app's shortcut and probe alternatives only on refusal. Registration dispatches to the UI
/// thread; callers must not hold locks needed by it. Empty disables even when no backend is available.
pub fn replace<R: Runtime>(app: &AppHandle<R>, accelerator: &str) -> MuteShortcut {
    replace_with::<R, Os>(app, accelerator)
}

/// Whether the host currently reports its accessibility reservation for this chord (VoiceOver on macOS).
pub fn reserved_chord_active(chord: &str) -> bool {
    Os::reserved_chord_active(chord)
}

fn replace_with<R: Runtime, N: Native>(app: &AppHandle<R>, accelerator: &str) -> MuteShortcut {
    let shortcuts = app.try_state::<GlobalShortcut<R>>();
    if let Some(shortcuts) = &shortcuts {
        let _ = shortcuts.unregister_all();
    }
    let (state, alternatives) = if accelerator.is_empty() {
        (Registration::Off, vec![])
    } else if let Some(shortcuts) = shortcuts {
        if let Err(error) = shortcuts.register(accelerator) {
            log::debug!("shortcut {accelerator} refused: {error}");
            let alternatives = settings::shortcut_alternatives(accelerator)
                .filter(|a| shortcuts.register(*a).is_ok() && shortcuts.unregister(*a).is_ok())
                .map(str::to_owned)
                .collect();
            (Registration::Refused, alternatives)
        } else {
            (Registration::On, vec![])
        }
    } else {
        (Registration::Unavailable, vec![])
    };
    MuteShortcut::new(accelerator, state, alternatives, N::label_style() == LabelStyle::Symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Baseline;
    impl Native for Baseline {}

    #[test]
    fn validation_accepts_normalized_keys_and_rejects_unknown_keys() {
        assert_eq!(checked(" control+option+m ").as_deref(), Some("Ctrl+Alt+M"));
        assert_eq!(checked(""), Some(String::new()));
        assert!(checked("M").is_none());
        assert!(checked("Ctrl+UnknownKey").is_none());
    }

    #[test]
    fn missing_backend_is_unavailable_and_disabling_still_works() {
        let app = tauri::test::mock_app();
        let status = replace_with::<_, Baseline>(app.handle(), settings::DEFAULT_MUTE_SHORTCUT);
        assert_eq!(status.state, Registration::Unavailable);
        assert!(status.alternatives.is_empty());
        assert!(!status.voice_over);
        assert_eq!(status.label.as_deref(), Some("Ctrl+Alt+M"));
        assert_eq!(serde_json::to_value(&status).unwrap()["state"], "unavailable");
        assert_eq!(replace_with::<_, Baseline>(app.handle(), ""), MuteShortcut::default());
    }

    #[test]
    fn label_styles_preserve_the_accelerator_and_accessibility_is_not_fabricated() {
        struct SymbolLabels;
        impl Native for SymbolLabels {
            fn label_style() -> LabelStyle {
                LabelStyle::Symbols
            }
        }
        let app = tauri::test::mock_app();
        assert_eq!(replace_with::<_, SymbolLabels>(app.handle(), "Ctrl+Alt+M").label.as_deref(), Some("⌃⌥M"));
        assert_eq!(replace_with::<_, Baseline>(app.handle(), "Ctrl+Alt+M").label.as_deref(), Some("Ctrl+Alt+M"));
        assert!(!<Baseline as Native>::reserved_chord_active("Ctrl+Alt+M"));
        assert!(!reserved_chord_active("Shift+M"));
    }
}
