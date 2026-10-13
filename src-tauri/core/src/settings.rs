//! What this device remembers: the mute shortcut, how the call controls card shows its controls, and how the app's
//! updates arrive. Which machine the interface talks to, and how to reach it, comes from pairing, never from here
//! (docs/TARGETS.md).
//!
//! Pure logic plus a JSON file in the app's config directory. No Tauri types here, so it is unit-tested on any
//! machine.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use url::Url;

/// Global mute toggle: ⌃⌥M on macOS, Ctrl+Alt+M elsewhere; editable in settings. Not ⌘⇧M: a global shortcut takes the
/// keys from every app, and ⌘⇧M is the Problems panel of VS Code (sidevoice/sidevoice-desktop#4).
pub const DEFAULT_MUTE_SHORTCUT: &str = "Ctrl+Alt+M";

pub const FILE_NAME: &str = "settings.json";

/// The origin Tauri serves the app's own pages from: what a node sees as the interface's `Origin`.
#[cfg(windows)]
pub const APP_ORIGIN: &str = "http://tauri.localhost";
#[cfg(not(windows))]
pub const APP_ORIGIN: &str = "tauri://localhost";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The mute shortcut as [`normalize_shortcut`] writes it; empty disables it.
    #[serde(default = "default_shortcut")]
    pub mute_shortcut: String,
    /// "Show the call controls always": the card keeps its controls in view instead of showing them near the pointer.
    #[serde(default)]
    pub call_controls_always: bool,
    /// How the app's updates arrive (core `updater`): automatic (the default) or manual. Set only by the room page
    /// (`host.updater.setMode`); the settings window neither shows nor sends it, and saving it keeps the stored one.
    #[serde(default)]
    pub update_mode: crate::updater::Mode,
}

fn default_shortcut() -> String {
    DEFAULT_MUTE_SHORTCUT.to_string()
}

/// What the app offers, in this order, when the system refuses the mute shortcut: those of these it can register
/// (checked against the systems' default lists: docs/SHORTCUTS.md).
pub const SHORTCUT_ALTERNATIVES: [&str; 2] = ["Ctrl+Alt+Shift+M", "Ctrl+Alt+Shift+K"];

/// The alternatives to offer for `refused`: never the refused one itself.
pub fn shortcut_alternatives(refused: &str) -> impl Iterator<Item = &'static str> + '_ {
    let refused = shortcut_keys(refused);
    SHORTCUT_ALTERNATIVES.into_iter().filter(move |a| shortcut_keys(a) != refused)
}

/// Whether the shortcut holds Control and Option together: VoiceOver's own keys on macOS (its "VO" modifier), so with
/// VoiceOver on it may get the keys first.
pub fn uses_voice_over_keys(accelerator: &str) -> bool {
    let keys = shortcut_keys(accelerator);
    keys.iter().any(|k| k == "ctrl") && keys.iter().any(|k| k == "alt")
}

/// An accelerator's keys, lower case, with each modifier under one name, in a stable order.
fn shortcut_keys(accelerator: &str) -> Vec<String> {
    let mut keys: Vec<String> = accelerator
        .split('+')
        .map(|k| k.trim().to_ascii_lowercase())
        .filter(|k| !k.is_empty())
        .map(|k| match k.as_str() {
            "control" => "ctrl".to_string(),
            "option" => "alt".to_string(),
            "command" | "cmd" | "super" | "meta" => "super".to_string(),
            _ => k,
        })
        .collect();
    keys.sort();
    keys
}

/// The modifiers a shortcut may hold, in the order the app writes them, each with the names it is read by.
const MODIFIERS: [[&str; 2]; 4] = [["Cmd", "Command"], ["Ctrl", "Control"], ["Alt", "Option"], ["Shift", "Shift"]];

/// The mute shortcut as the room page (`host.muteShortcut.set`, sidevoice-web's `Cmd+Shift+M`) and the settings window
/// write it: `+`-separated, any case, one or more of the modifiers `Cmd`/`Command`, `Ctrl`/`Control`, `Alt`/`Option`
/// and `Shift`, then one key (`M`, `F5`, `Space`). Answered in the one form the app keeps, `Cmd+Ctrl+Alt+Shift+Key`
/// (those held, in that order; the key's first letter upper case). Empty is no shortcut; `None`, not a shortcut.
/// Whether the system knows the key is the shell's to ask (global-hotkey, which reads `Cmd` as Super).
pub fn normalize_shortcut(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return Some(String::new());
    }
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let key = parts.pop()?;
    let modifier = |part: &str| MODIFIERS.iter().position(|names| names.iter().any(|n| n.eq_ignore_ascii_case(part)));
    let mut held = [false; MODIFIERS.len()];
    for part in parts {
        held[modifier(part)?] = true;
    }
    if !held.contains(&true) || key.is_empty() || modifier(key).is_some() {
        return None;
    }
    let mut chars = key.chars();
    let key: String = chars.next()?.to_uppercase().chain(chars).collect();
    let names = MODIFIERS.iter().zip(held).filter(|(_, on)| *on).map(|(names, _)| names[0]);
    Some(names.chain([key.as_str()]).collect::<Vec<_>>().join("+"))
}

/// Whether the system took the mute shortcut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Registration {
    /// None set.
    #[default]
    Off,
    On,
    /// The system would not register it: another app may hold it.
    Refused,
}

/// The mute shortcut as the system took it: `host.muteShortcut`'s `MuteShortcut` (docs/BRIDGE.md → "The mute
/// shortcut"), which the settings window shows too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MuteShortcut {
    /// As the app keeps it ([`normalize_shortcut`]); `None` when there is none.
    pub accelerator: Option<String>,
    /// How it reads on this system: ⌃⌥M, Ctrl+Alt+M.
    pub label: Option<String>,
    pub state: Registration,
    /// Refused: those of [`SHORTCUT_ALTERNATIVES`] the system does take, to offer instead.
    pub alternatives: Vec<String>,
    /// macOS: VoiceOver is on, and the shortcut holds its keys (Control and Option together).
    pub voice_over: bool,
}

impl MuteShortcut {
    /// `accelerator` (empty: none, `off`) as the system took it, read as macOS (`macos`) or the others write it.
    pub fn new(accelerator: &str, state: Registration, alternatives: Vec<String>, macos: bool) -> MuteShortcut {
        if accelerator.is_empty() {
            return MuteShortcut::default();
        }
        MuteShortcut {
            accelerator: Some(accelerator.to_owned()),
            label: Some(crate::call_controls::shortcut_label(accelerator, macos)),
            state,
            alternatives,
            voice_over: false,
        }
    }
}

impl Settings {
    /// First run: the default shortcut.
    pub fn first_run() -> Self {
        Settings {
            mute_shortcut: default_shortcut(),
            call_controls_always: false,
            update_mode: crate::updater::Mode::Automatic,
        }
    }
}

/// An URL's origin as a page on it reports it. The WHATWG origin of a custom scheme (`tauri://localhost`) is
/// opaque (`null`), but WebKit and WebView2 report the app's pages as `scheme://host`, and so does this.
pub fn url_origin(url: &Url) -> String {
    match url.scheme() {
        "http" | "https" => url.origin().ascii_serialization(),
        scheme => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{scheme}://{host}:{port}"),
            (Some(host), None) => format!("{scheme}://{host}"),
            _ => "null".to_string(),
        },
    }
}

/// `None` when there is no file yet (first run) or it cannot be read as settings.
pub fn load(dir: &Path) -> Option<Settings> {
    let raw = fs::read_to_string(dir.join(FILE_NAME)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save(dir: &Path, settings: &Settings) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(settings).expect("settings serialise"))?;
    fs::rename(tmp, dir.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_refused_shortcut_has_alternatives_but_never_itself() {
        let offered: Vec<_> = shortcut_alternatives("Control+Option+Shift+M").collect();
        assert_eq!(offered, ["Ctrl+Alt+Shift+K"]);
        assert_eq!(shortcut_alternatives(DEFAULT_MUTE_SHORTCUT).next(), Some("Ctrl+Alt+Shift+M"));
    }

    #[test]
    fn control_and_option_together_are_voice_overs_keys() {
        assert!(uses_voice_over_keys(DEFAULT_MUTE_SHORTCUT));
        assert!(uses_voice_over_keys("Option+Control+Shift+M"));
        assert!(!uses_voice_over_keys("Cmd+Shift+M"));
        assert!(!uses_voice_over_keys(""));
    }

    #[test]
    fn the_web_s_shortcuts_are_kept_in_one_form() {
        assert_eq!(normalize_shortcut("Cmd+Shift+M").as_deref(), Some("Cmd+Shift+M"));
        assert_eq!(normalize_shortcut(" shift + command+m ").as_deref(), Some("Cmd+Shift+M"));
        assert_eq!(normalize_shortcut("Option+Control+f5").as_deref(), Some("Ctrl+Alt+F5"));
        assert_eq!(normalize_shortcut("CTRL+ALT+space").as_deref(), Some("Ctrl+Alt+Space"));
        assert_eq!(normalize_shortcut("Alt+Ctrl+Ctrl+;").as_deref(), Some("Ctrl+Alt+;"), "a modifier held twice");
        assert_eq!(normalize_shortcut(DEFAULT_MUTE_SHORTCUT).as_deref(), Some(DEFAULT_MUTE_SHORTCUT));
        for alternative in SHORTCUT_ALTERNATIVES {
            assert_eq!(normalize_shortcut(alternative).as_deref(), Some(alternative), "already in the kept form");
        }
        assert_eq!(normalize_shortcut("  ").as_deref(), Some(""), "empty: none");
    }

    #[test]
    fn what_is_not_a_shortcut() {
        for text in
            ["M", "F5", "Cmd", "Cmd+Shift", "Cmd+", "+M", "Cmd++M", "Cmd+M+K", "Hyper+M", "M+Cmd", "CmdOrCtrl+M"]
        {
            assert_eq!(normalize_shortcut(text), None, "{text}");
        }
    }

    #[test]
    fn the_mute_shortcut_reads_as_the_seam_says() {
        assert_eq!(
            serde_json::to_value(MuteShortcut::new("", Registration::On, vec![], true)).unwrap(),
            json!({"accelerator": null, "label": null, "state": "off", "alternatives": [], "voiceOver": false})
        );
        assert_eq!(
            serde_json::to_value(MuteShortcut::new("Ctrl+Alt+M", Registration::On, vec![], true)).unwrap(),
            json!({"accelerator": "Ctrl+Alt+M", "label": "⌃⌥M", "state": "on", "alternatives": [], "voiceOver": false})
        );
        let refused = MuteShortcut::new("Cmd+Shift+M", Registration::Refused, vec!["Ctrl+Alt+Shift+M".into()], false);
        assert_eq!(
            serde_json::to_value(refused).unwrap(),
            json!({"accelerator": "Cmd+Shift+M", "label": "Super+Shift+M", "state": "refused",
                   "alternatives": ["Ctrl+Alt+Shift+M"], "voiceOver": false})
        );
    }

    #[test]
    fn custom_scheme_origins_are_scheme_and_host() {
        assert_eq!(url_origin(&Url::parse("tauri://localhost/voice/index.html").unwrap()), "tauri://localhost");
        assert_eq!(url_origin(&Url::parse("http://tauri.localhost/voice/").unwrap()), "http://tauri.localhost");
        assert_eq!(
            url_origin(&Url::parse("https://node.example.com:8443/a").unwrap()),
            "https://node.example.com:8443"
        );
        assert_eq!(url_origin(&Url::parse("about:blank").unwrap()), "null");
    }

    #[test]
    fn round_trip_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("sidevoice-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), None, "first run: nothing stored");

        let s = Settings {
            mute_shortcut: "Alt+M".into(),
            call_controls_always: true,
            update_mode: crate::updater::Mode::Manual,
        };
        save(&dir, &s).unwrap();
        assert_eq!(load(&dir), Some(s));

        fs::write(dir.join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(load(&dir), None, "corrupt file: start as a first run rather than guess");

        fs::write(dir.join(FILE_NAME), "{}").unwrap();
        assert_eq!(load(&dir), Some(Settings::first_run()), "what is missing takes its default");
        fs::remove_dir_all(&dir).unwrap();
    }
}
