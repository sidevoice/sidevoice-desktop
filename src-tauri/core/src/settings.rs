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
    /// "Always show the call controls": the call controls card shows during a call even while the room's window is in
    /// front; off, only while it is behind or minimised (core `call_controls::wanted`).
    #[serde(default)]
    pub call_controls_always_shown: bool,
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
const SHIFT: usize = 3;

/// The characters Shift makes of a key on a US keyboard, with that key: what the web's recorder writes for
/// Ctrl+Shift+/ is `Ctrl+Shift+?`, which the system knows as Ctrl+Shift+Slash.
const SHIFTED: [(char, char); 21] = [
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
];

/// The mute shortcut as the room page (`host.muteShortcut.set`, sidevoice-web's recorder) and the settings window
/// write it: `+`-separated, any case, one or more of the modifiers `Cmd`/`Command`, `Ctrl`/`Control`, `Alt`/`Option`
/// and `Shift`, then one key as the browser names it (`M`, `5`, `F5`, `Space`, `ArrowUp`, `;`, `+`). With Shift, a
/// character Shift makes is read as its key ([`SHIFTED`]: `Ctrl+Shift++` is Ctrl+Shift+=). Answered in the one form the
/// app keeps, `Cmd+Ctrl+Alt+Shift+Key` (those held, in that order; the key's first letter upper case). Empty is no
/// shortcut; `None`, not a shortcut. Whether the system knows the key is the shell's to ask (global-hotkey, which
/// reads `Cmd` as Super).
pub fn normalize_shortcut(text: &str) -> Option<String> {
    let mut rest = text.trim();
    if rest.is_empty() {
        return Some(String::new());
    }
    let modifier = |part: &str| MODIFIERS.iter().position(|names| names.iter().any(|n| n.eq_ignore_ascii_case(part)));
    let mut held = [false; MODIFIERS.len()];
    while let Some(at) = rest.split_once('+').and_then(|(head, tail)| Some((modifier(head.trim())?, tail))) {
        held[at.0] = true;
        rest = at.1;
    }
    let key = rest.trim();
    if !held.contains(&true) || modifier(key).is_some() {
        return None;
    }
    let key = native_key(key, held[SHIFT])?;
    let names = MODIFIERS.iter().zip(held).filter(|(_, on)| *on).map(|(names, _)| names[0]);
    Some(names.chain([key.as_str()]).collect::<Vec<_>>().join("+"))
}

/// One key as the browser names it, as the app writes it: one character upper case (a character Shift made, its key);
/// a name (`F5`, `space`, `ArrowUp`) with its first letter upper case. `None` for no key, or more than one.
fn native_key(key: &str, shift: bool) -> Option<String> {
    let mut chars = key.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        let unshifted = SHIFTED.iter().find(|(made, _)| shift && *made == first).map_or(first, |(_, key)| *key);
        return Some(unshifted.to_uppercase().collect());
    }
    if key.contains(['+', ' ']) {
        return None;
    }
    Some(first.to_uppercase().chain(key.chars().skip(1)).collect())
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

/// What the settings window changed: only what the person changed there, so a value set meanwhile from the room page
/// is never overwritten by the one the window had loaded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowChanges {
    /// Already checked by the shell (a shortcut it can register, in the form [`normalize_shortcut`] keeps).
    pub mute_shortcut: Option<String>,
    pub call_controls_always_shown: Option<bool>,
}

impl WindowChanges {
    pub fn apply(self, to: &mut Settings) {
        if let Some(accelerator) = self.mute_shortcut {
            to.mute_shortcut = accelerator;
        }
        if let Some(always) = self.call_controls_always_shown {
            to.call_controls_always_shown = always;
        }
    }
}

impl Settings {
    /// First run: the default shortcut.
    pub fn first_run() -> Self {
        Settings {
            mute_shortcut: default_shortcut(),
            call_controls_always_shown: false,
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

    /// What sidevoice-web's recorder writes (`acceleratorOf`: the modifiers, then `event.key`, a single character
    /// upper case, `" "` as `Space`), through the app's form, to the parser the app registers shortcuts with.
    #[test]
    fn what_the_web_s_recorder_writes_registers_as_its_keys() {
        use global_hotkey::hotkey::{Code, HotKey, Modifiers};
        let (cmd, ctrl, alt, shift) = (Modifiers::SUPER, Modifiers::CONTROL, Modifiers::ALT, Modifiers::SHIFT);
        for (recorded, kept, mods, code) in [
            ("Cmd+Shift+M", "Cmd+Shift+M", cmd | shift, Code::KeyM),
            ("Ctrl+Alt+M", "Ctrl+Alt+M", ctrl | alt, Code::KeyM),
            ("Cmd+Shift++", "Cmd+Shift+=", cmd | shift, Code::Equal),
            ("Ctrl+=", "Ctrl+=", ctrl, Code::Equal),
            ("Ctrl+Shift+?", "Ctrl+Shift+/", ctrl | shift, Code::Slash),
            ("Ctrl+Shift+!", "Ctrl+Shift+1", ctrl | shift, Code::Digit1),
            ("Cmd+Shift+_", "Cmd+Shift+-", cmd | shift, Code::Minus),
            ("Ctrl+Shift+:", "Ctrl+Shift+;", ctrl | shift, Code::Semicolon),
            ("Ctrl+Alt+;", "Ctrl+Alt+;", ctrl | alt, Code::Semicolon),
            ("Cmd+7", "Cmd+7", cmd, Code::Digit7),
            ("Cmd+F5", "Cmd+F5", cmd, Code::F5),
            ("Ctrl+F13", "Ctrl+F13", ctrl, Code::F13),
            ("Ctrl+Alt+Space", "Ctrl+Alt+Space", ctrl | alt, Code::Space),
            ("Cmd+ArrowUp", "Cmd+ArrowUp", cmd, Code::ArrowUp),
            ("Ctrl+Shift+PageDown", "Ctrl+Shift+PageDown", ctrl | shift, Code::PageDown),
            ("Alt+Enter", "Alt+Enter", alt, Code::Enter),
        ] {
            let normalized = normalize_shortcut(recorded);
            assert_eq!(normalized.as_deref(), Some(kept), "{recorded}");
            let hotkey: HotKey = kept.parse().unwrap_or_else(|e| panic!("{recorded} → {kept}: {e}"));
            assert_eq!((hotkey.mods, hotkey.key), (mods, code), "{recorded}");
        }
        // What no key on the keyboard is, as the parser says: the shell refuses these `shortcut_invalid`.
        for recorded in ["Cmd+Alt+µ", "Ctrl+Dead", "Ctrl+Unidentified", "Ctrl+É"] {
            let kept = normalize_shortcut(recorded).unwrap();
            assert!(kept.parse::<HotKey>().is_err(), "{recorded} → {kept}");
        }
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
    fn the_settings_window_never_undoes_a_change_made_from_the_page() {
        // The window loaded the defaults; meanwhile the page turned "always shown" on; the window saved its shortcut.
        let mut stored = Settings::first_run();
        stored.call_controls_always_shown = true;
        WindowChanges { mute_shortcut: Some("Cmd+Shift+M".into()), ..WindowChanges::default() }.apply(&mut stored);
        assert_eq!((stored.mute_shortcut.as_str(), stored.call_controls_always_shown), ("Cmd+Shift+M", true));
        // The other way round: the page set a shortcut; the window saved its checkbox.
        let mut stored = Settings::first_run();
        stored.mute_shortcut = "Ctrl+Alt+K".into();
        WindowChanges { call_controls_always_shown: Some(true), ..WindowChanges::default() }.apply(&mut stored);
        assert_eq!((stored.mute_shortcut.as_str(), stored.call_controls_always_shown), ("Ctrl+Alt+K", true));
        // Nothing changed there: nothing changes, the updates' mode included.
        let mut stored = Settings { update_mode: crate::updater::Mode::Manual, ..Settings::first_run() };
        let before = stored.clone();
        WindowChanges::default().apply(&mut stored);
        assert_eq!(stored, before);
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
            call_controls_always_shown: true,
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
