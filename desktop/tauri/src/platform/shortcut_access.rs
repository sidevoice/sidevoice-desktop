//! Global shortcut registration and the host's shortcut labels / accessibility reservation.
//!
//! The system's hot keys are registered through the maintained global-shortcut plugin; which of them the app holds follows
//! what the settings say by a diff (core `shortcuts::reconcile`), never by letting go of everything and registering again.

use sidevoice_desktop_core::shortcuts::{self, Action, Edge, Held, KeyReach, Parsed, Platform, Registrar, Taken};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcut, Shortcut, ShortcutState};

#[cfg_attr(target_os = "macos", path = "shortcut_access/macos.rs")]
#[cfg_attr(target_os = "windows", path = "shortcut_access/windows.rs")]
#[cfg_attr(target_os = "linux", path = "shortcut_access/linux.rs")]
mod imp;
use imp::Native;

#[derive(PartialEq, Eq)]
enum LabelStyle {
    Symbols,
    Names,
}

/// The only parts of shortcut handling that differ per OS; everything else is the plain functions below.
trait Os {
    /// The system this is, for the keys it can and cannot register (core `shortcuts::key_reach`).
    const PLATFORM: Platform;

    /// ⌃⌥M on macOS; elsewhere Ctrl+Alt+Shift+M (Ctrl+Alt+letter is AltGr on many Windows layouts).
    const DEFAULT_MUTE: &'static str = shortcuts::DEFAULT_MUTE_OTHER;

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

/// The system this build runs on.
pub fn platform() -> Platform {
    Native::PLATFORM
}

/// The mute shortcut this system starts with.
pub fn default_mute() -> &'static str {
    Native::DEFAULT_MUTE
}

/// Whether shortcuts read with symbols (⌃⌥M) on this system.
pub fn symbol_labels() -> bool {
    Native::label_style() == LabelStyle::Symbols
}

/// Whether the plugin knows the keys a binding registers as a hot key (a binding with none is a listener's).
pub fn accepts(parsed: &Parsed) -> bool {
    parsed.hot_key().is_none_or(|keys| keys.parse::<Shortcut>().is_ok())
}

/// What the app is told of a hot key: which action's, and whether it went down or up.
pub type HotKeySink = Arc<dyn Fn(&AppHandle, Action, Edge) + Send + Sync>;

/// Which action each registered hot key belongs to (the plugin hands the handler the key, not an id of ours).
#[derive(Default)]
struct Bindings(Mutex<Vec<(u32, Action)>>);

/// Install the maintained global-shortcut plugin on the UI thread. A missing backend must not stop startup.
pub fn configure(app: &AppHandle, sink: HotKeySink) {
    if !Native::backend_available() {
        log::info!("global shortcuts unavailable in this desktop session");
        return;
    }
    app.manage(Bindings::default());
    let plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, shortcut, event| {
            let id = shortcut.id();
            let action = app
                .try_state::<Bindings>()
                .and_then(|b| b.0.lock().unwrap().iter().find(|(held, _)| *held == id).map(|(_, a)| *a));
            let edge = match event.state() {
                ShortcutState::Pressed => Edge::Down,
                ShortcutState::Released => Edge::Up,
            };
            if let Some(action) = action {
                sink(app, action, edge);
            }
        })
        .build();
    if let Err(error) = app.plugin(plugin) {
        log::warn!("global shortcuts unavailable: {error}");
    }
}

/// The plugin's registrations, as the diff asks for them.
struct Plugin<'a, R: Runtime>(&'a GlobalShortcut<R>);

impl<R: Runtime> Registrar for Plugin<'_, R> {
    fn register(&mut self, hot_key: &str) -> bool {
        // The backend would bind another key for these (Windows: Ctrl+NumpadEqual as Ctrl+E) or none.
        if shortcuts::hot_key_reach(platform(), hot_key) == KeyReach::Never {
            log::debug!("shortcut {hot_key} not registered: this system cannot register that key");
            return false;
        }
        match self.0.register(hot_key) {
            Ok(()) => true,
            Err(error) => {
                log::debug!("shortcut {hot_key} not registered: {error}");
                false
            }
        }
    }

    fn unregister(&mut self, hot_key: &str) -> bool {
        self.0.unregister(hot_key).is_ok()
    }
}

/// Without a backend nothing can be registered or held.
struct Absent;

impl Registrar for Absent {
    fn register(&mut self, _hot_key: &str) -> bool {
        false
    }

    fn unregister(&mut self, _hot_key: &str) -> bool {
        false
    }
}

/// Bring the app's hot keys to `wanted` (one per action; `None`: none, or a key a listener carries), touching only what
/// changed, and say how the system took each. Registration dispatches to the UI thread; callers must not hold locks needed
/// by it or by the hot key handler. Disabling works even when no backend is available.
pub fn replace<R: Runtime>(app: &AppHandle<R>, held: &mut Held, wanted: &[Option<String>; 3]) -> [Option<Taken>; 3] {
    let plugin = app.try_state::<GlobalShortcut<R>>();
    let taken = match &plugin {
        Some(plugin) => shortcuts::reconcile(&mut Plugin(plugin), held, wanted, true),
        None => shortcuts::reconcile(&mut Absent, held, wanted, false),
    };
    if let Some(bindings) = app.try_state::<Bindings>() {
        *bindings.0.lock().unwrap() = Action::ALL
            .into_iter()
            .filter_map(|action| {
                let shortcut: Shortcut = held.of(action)?.parse().ok()?;
                Some((shortcut.id(), action))
            })
            .collect();
    }
    taken
}

/// What is offered in place of a key a listener carries that another action owns: the system's keys no action asks for.
pub fn alternatives<R: Runtime>(app: &AppHandle<R>, wanted: &[Option<String>; 3]) -> Vec<String> {
    match app.try_state::<GlobalShortcut<R>>() {
        Some(plugin) => shortcuts::alternatives_for(&mut Plugin(&plugin), wanted, ""),
        None => Vec::new(),
    }
}

/// Whether the host currently reports its accessibility reservation for this chord (VoiceOver on macOS).
pub fn reserved_chord_active(chord: &str) -> bool {
    Native::reserved_chord_active(chord)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_desktop_core::settings::Registration;

    fn parsed(text: &str) -> Parsed {
        shortcuts::parse(text).unwrap().unwrap()
    }

    #[test]
    fn the_plugin_knows_the_keys_a_binding_registers() {
        for known in ["Ctrl+Alt+M", "Ctrl+Alt+Shift+M", "Cmd+Shift+M", "F9", "F13:tap", "F24:hold", "Ctrl+Alt+Space"] {
            assert!(accepts(&parsed(known)), "{known}");
        }
        // Not a hot key at all: a listener's, so nothing for the plugin to refuse.
        assert!(accepts(&parsed("Fn:hold")) && accepts(&parsed("RightControl:double")));
        assert!(!accepts(&Parsed::Combo("Ctrl+UnknownKey".into())));
    }

    #[test]
    fn this_system_starts_with_its_own_mute_default() {
        let default = default_mute();
        assert!(default == shortcuts::DEFAULT_MUTE_MACOS || default == shortcuts::DEFAULT_MUTE_OTHER);
        assert_eq!(symbol_labels(), default == shortcuts::DEFAULT_MUTE_MACOS, "symbols and ⌃⌥M are macOS's together");
        assert!(accepts(&parsed(default)));
    }

    #[test]
    fn missing_backend_is_unavailable_and_disabling_still_works() {
        let app = tauri::test::mock_app();
        let mut held = Held::default();
        let wanted = [Some("Ctrl+Alt+Shift+M".to_string()), None, Some("F9".to_string())];
        let taken = replace(app.handle(), &mut held, &wanted);
        assert_eq!(taken[0].as_ref().map(|t| t.state), Some(Registration::Unavailable));
        assert!(taken[1].is_none());
        assert_eq!(taken[2].as_ref().map(|t| t.state), Some(Registration::Unavailable));
        assert_eq!(serde_json::to_value(Registration::Unavailable).unwrap(), "unavailable");
        assert_eq!(serde_json::to_value(Registration::NeedsPermission).unwrap(), "needsPermission");
        let none = replace(app.handle(), &mut held, &[None, None, None]);
        assert!(none.iter().all(Option::is_none));
    }

    #[test]
    fn accessibility_is_not_fabricated() {
        struct Baseline;
        impl Os for Baseline {
            const PLATFORM: Platform = Platform::Linux;
        }
        assert!(!<Baseline as Os>::reserved_chord_active("Ctrl+Alt+M"));
        assert!(!<Baseline as Os>::reserved_chord_active("Shift+M"));
        assert!(<Baseline as Os>::label_style() == LabelStyle::Names);
    }
}
