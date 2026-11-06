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

pub const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The mute shortcut as [`crate::shortcuts::parse`] writes it; empty disables it. A file that has none is from before
    /// the shortcut could be set from the page, and holds the shortcut every system registered then.
    #[serde(default = "legacy_shortcut")]
    pub mute_shortcut: String,
    /// The hold to talk shortcut (`host.shortcuts`); empty: none, the default.
    #[serde(default)]
    pub hold_to_talk_shortcut: String,
    /// The say one thing shortcut (`host.shortcuts`); empty: none, the default.
    #[serde(default)]
    pub one_turn_shortcut: String,
    /// Whether the mute shortcut has been through the move off its old default ([`crate::shortcuts::migrated_mute`]): once.
    #[serde(default)]
    pub shortcuts_migrated: bool,
    /// What a headset's single press does where the app answers one (`host.shortcuts.setHeadset`).
    #[serde(default)]
    pub headset_press: crate::shortcuts::HeadsetPress,
    /// What a headset's double press does (it arrives as the system's next-track command): nothing by default, so that a
    /// keyboard's next-track key, which arrives the same way, does nothing until the person chooses (`host.shortcuts.setHeadset`).
    #[serde(default = "default_double_press")]
    pub headset_double_press: crate::shortcuts::HeadsetPress,
    /// Whether the person chose Allow for Input Monitoring: the system says only granted or not, and this tells a
    /// permission never asked for from one refused.
    #[serde(default)]
    pub input_monitoring_asked: bool,
    /// "Always show the call controls": the call controls card shows during a call even while the room's window is in
    /// front; off, only while it is behind or minimised (core `call_controls::wanted`).
    #[serde(default)]
    pub call_controls_always_shown: bool,
    /// "Show the floating card": on by default; off, no card shows during a call, not even the moment a change in Settings
    /// shows it (`host.callControls.setEnabled`).
    #[serde(default = "default_true")]
    pub call_controls_enabled: bool,
    /// How the app's updates arrive (core `updater`): automatic (the default) or manual. Set only by the room page
    /// (`host.updater.setMode`).
    #[serde(default)]
    pub update_mode: crate::updater::Mode,
    /// Where a floating card that was never moved on a display first appears (`host.callControls.setCorner`): the top
    /// right by default.
    #[serde(default)]
    pub call_controls_corner: crate::call_controls::Corner,
    /// The developer options' switches (`host.debug`; nightly and dev builds): all off by default.
    #[serde(default)]
    pub debug: crate::debug_options::DebugOptions,
    /// What happens to other apps' media while the microphone is open in a call (`host.mediaDuck`; docs/MEDIA_DUCK.md):
    /// lowered by default.
    #[serde(default)]
    pub media_duck: crate::media_duck::Mode,
    /// How far Lower takes other apps (`host.mediaDuck.setLevel`): a quarter of their volume by default.
    #[serde(default)]
    pub media_duck_level: crate::media_duck::Level,
}

fn default_true() -> bool {
    true
}

fn default_double_press() -> crate::shortcuts::HeadsetPress {
    crate::shortcuts::HeadsetPress::None
}

fn legacy_shortcut() -> String {
    crate::shortcuts::LEGACY_MUTE_SHORTCUT.to_string()
}

/// Whether two accelerators are the same keys, however each is written.
pub fn same_shortcut(a: &str, b: &str) -> bool {
    match (normalize_shortcut(a), normalize_shortcut(b)) {
        (Some(a), Some(b)) => a == b,
        _ => shortcut_keys(a) == shortcut_keys(b),
    }
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

/// The mute shortcut as the room page (`host.muteShortcut.set`, sidevoice-web's recorder)
/// writes it: `+`-separated, any case, one or more of the modifiers `Cmd`/`Command`, `Ctrl`/`Control`, `Alt`/`Option`
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
    if modifier(key).is_some() {
        return None;
    }
    let key = native_key(key, held[SHIFT])?;
    // A key on its own would be taken from every app: only an F-key may be one.
    if !held.contains(&true) && !is_function_key(&key) {
        return None;
    }
    let names = MODIFIERS.iter().zip(held).filter(|(_, on)| *on).map(|(names, _)| names[0]);
    Some(names.chain([key.as_str()]).collect::<Vec<_>>().join("+"))
}

/// `F1` to `F24`.
fn is_function_key(key: &str) -> bool {
    key.strip_prefix('F').is_some_and(|n| !n.starts_with('0') && n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)))
}

/// The keys the app names by the physical key, never by the character they type, as `global-hotkey` names them: the main
/// row's punctuation and the keypad. A character can be two keys (`-` is the main row's and the keypad's) and `+` is the
/// accelerator's own separator; a name is one key, on any layout.
const PHYSICAL_KEYS: [&str; 29] = [
    "Minus",
    "Equal",
    "Comma",
    "Period",
    "Slash",
    "Backslash",
    "Semicolon",
    "Quote",
    "BracketLeft",
    "BracketRight",
    "Backquote",
    "IntlBackslash",
    "NumpadAdd",
    "NumpadSubtract",
    "NumpadMultiply",
    "NumpadDivide",
    "NumpadDecimal",
    "NumpadEqual",
    "NumpadEnter",
    "Numpad0",
    "Numpad1",
    "Numpad2",
    "Numpad3",
    "Numpad4",
    "Numpad5",
    "Numpad6",
    "Numpad7",
    "Numpad8",
    "Numpad9",
];

/// How a physical key reads on a keycap: `-` for `Minus`, `Num +` for `NumpadAdd`; `None` for any other name.
pub fn keycap(name: &str) -> Option<String> {
    let key = PHYSICAL_KEYS.iter().find(|known| known.eq_ignore_ascii_case(name))?;
    if let Some(on_pad) = key.strip_prefix("Numpad") {
        let symbol = match on_pad {
            "Add" => "+",
            "Subtract" => "-",
            "Multiply" => "*",
            "Divide" => "/",
            "Decimal" => ".",
            "Equal" => "=",
            other => other,
        };
        return Some(format!("Num {symbol}"));
    }
    let character = match *key {
        "Minus" => "-",
        "Equal" => "=",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Quote" => "'",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "IntlBackslash" => "<",
        _ => "`",
    };
    Some(character.to_owned())
}

/// The key a character has always meant in an accelerator saved before keys were named by the physical key: the main row's.
/// `+` and `*` alone were never a main-row key (they need Shift there), so they were the keypad's.
fn legacy_key(character: char, shift: bool) -> Option<&'static str> {
    Some(match character {
        '-' => "Minus",
        '=' => "Equal",
        ',' => "Comma",
        '.' => "Period",
        '/' => "Slash",
        '\\' => "Backslash",
        ';' => "Semicolon",
        '\'' => "Quote",
        '[' => "BracketLeft",
        ']' => "BracketRight",
        '`' => "Backquote",
        '+' if !shift => "NumpadAdd",
        '*' if !shift => "NumpadMultiply",
        _ => return None,
    })
}

/// One key as the page or an older version writes it, as the app writes it: a physical key by its name (`-` is `Minus`,
/// `NumpadSubtract` stays the keypad's; a character Shift made, its key); any other single character upper case; any other
/// name with its first letter upper case. `None` for no key, or more than one.
fn native_key(key: &str, shift: bool) -> Option<String> {
    let mut chars = key.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        let unshifted = SHIFTED.iter().find(|(made, _)| shift && *made == first).map_or(first, |(_, key)| *key);
        if let Some(name) = legacy_key(unshifted, shift) {
            return Some(name.to_owned());
        }
        return Some(unshifted.to_uppercase().collect());
    }
    if key.contains(['+', ' ']) {
        return None;
    }
    if key.eq_ignore_ascii_case("return") {
        return Some("Enter".to_owned());
    }
    if let Some(known) = PHYSICAL_KEYS.iter().find(|known| known.eq_ignore_ascii_case(key)) {
        return Some((*known).to_owned());
    }
    Some(first.to_uppercase().chain(key.chars().skip(1)).collect())
}

/// Whether the system took a global shortcut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Registration {
    /// None set.
    #[default]
    Off,
    On,
    /// The system would not register it: another app may hold it, or the same keys are set for two actions.
    Refused,
    /// A single key waits for a permission the system has not given (macOS: Input Monitoring).
    NeedsPermission,
    /// No global shortcut backend is available in this desktop session.
    Unavailable,
}

/// A global shortcut as the system took it: `host.shortcuts`' `GlobalShortcut`, and `host.muteShortcut`'s `MuteShortcut`
/// for the mute one (docs/BRIDGE.md → "The shortcuts").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalShortcut {
    /// As the app keeps it ([`crate::shortcuts::parse`]); `None` when there is none.
    pub accelerator: Option<String>,
    /// How it reads on this system: ⌃⌥M, Ctrl+Alt+M.
    pub label: Option<String>,
    pub state: Registration,
    /// Refused: those of [`crate::shortcuts::ALTERNATIVES`] the system does take and nothing else of ours holds, to offer.
    pub alternatives: Vec<String>,
    /// macOS: VoiceOver is on, and the shortcut holds its keys (Control and Option together).
    pub voice_over: bool,
    /// Linux (X11): the key registers by the character the layout types on it, not by its position, so under a layout other
    /// than US it may be another key, or none ([`crate::shortcuts::KeyReach::ByLayout`]). A warning, not a refusal.
    pub layout_bound: bool,
    /// The system owns the keys and the label is its text (a Wayland portal): never here, the keys are the app's.
    pub managed_by_system: bool,
}

impl GlobalShortcut {
    /// `accelerator` (empty: none, `off`) as the system took it, read as macOS (`macos`) or the others write it, in `language`.
    pub fn new(
        accelerator: &str,
        state: Registration,
        alternatives: Vec<String>,
        macos: bool,
        language: &str,
    ) -> GlobalShortcut {
        if accelerator.is_empty() {
            return GlobalShortcut::default();
        }
        GlobalShortcut {
            accelerator: Some(accelerator.to_owned()),
            label: Some(crate::shortcuts::label(accelerator, macos, language)),
            state,
            alternatives,
            voice_over: false,
            layout_bound: false,
            managed_by_system: false,
        }
    }
}

impl Settings {
    /// Moves a person still on the old mute default to `default_mute`, once ([`crate::shortcuts::migrated_mute`]).
    /// Whether anything changed, for the caller to keep it.
    pub fn migrate_shortcuts(&mut self, default_mute: &str) -> bool {
        if self.shortcuts_migrated {
            return false;
        }
        if let Some(moved) = crate::shortcuts::migrated_mute(&self.mute_shortcut, false, default_mute) {
            self.mute_shortcut = moved;
        }
        self.shortcuts_migrated = true;
        true
    }

    /// First run: `default_mute`, this system's default mute shortcut (`shortcut_access::default_mute`), and no other.
    pub fn first_run(default_mute: &str) -> Self {
        Settings {
            mute_shortcut: default_mute.to_owned(),
            hold_to_talk_shortcut: String::new(),
            one_turn_shortcut: String::new(),
            shortcuts_migrated: true,
            headset_press: crate::shortcuts::HeadsetPress::Mute,
            headset_double_press: crate::shortcuts::HeadsetPress::None,
            input_monitoring_asked: false,
            call_controls_always_shown: false,
            call_controls_enabled: true,
            update_mode: crate::updater::Mode::Automatic,
            call_controls_corner: crate::call_controls::Corner::TopRight,
            debug: crate::debug_options::DebugOptions::default(),
            media_duck: crate::media_duck::Mode::Lower,
            media_duck_level: crate::media_duck::Level::Quarter,
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
    fn shortcuts_are_the_same_however_they_are_written() {
        assert!(same_shortcut("Control+Option+Shift+M", "Ctrl+Alt+Shift+M"));
        assert!(same_shortcut("shift+alt+ctrl+m", "Ctrl+Alt+Shift+M"));
        assert!(!same_shortcut("Ctrl+Alt+M", "Ctrl+Alt+Shift+M"));
    }

    #[test]
    fn control_and_option_together_are_voice_overs_keys() {
        assert!(uses_voice_over_keys(crate::shortcuts::DEFAULT_MUTE_MACOS));
        assert!(uses_voice_over_keys(crate::shortcuts::DEFAULT_MUTE_OTHER));
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
        assert_eq!(
            normalize_shortcut("Alt+Ctrl+Ctrl+;").as_deref(),
            Some("Ctrl+Alt+Semicolon"),
            "a modifier held twice"
        );
        for kept in [crate::shortcuts::DEFAULT_MUTE_MACOS, crate::shortcuts::DEFAULT_MUTE_OTHER] {
            assert_eq!(normalize_shortcut(kept).as_deref(), Some(kept), "a default is in the kept form");
        }
        for alternative in crate::shortcuts::ALTERNATIVES {
            assert_eq!(normalize_shortcut(alternative).as_deref(), Some(alternative), "already in the kept form");
        }
        assert_eq!(normalize_shortcut("  ").as_deref(), Some(""), "empty: none");
        // A key on its own is only an F-key.
        assert_eq!(normalize_shortcut("F9").as_deref(), Some("F9"));
        assert_eq!(normalize_shortcut("f13").as_deref(), Some("F13"));
        assert_eq!(normalize_shortcut("F24").as_deref(), Some("F24"));
        for text in ["F25", "F0", "F05", "Fn", "Space", "7"] {
            assert_eq!(normalize_shortcut(text), None, "{text}");
        }
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
            ("Cmd+Shift++", "Cmd+Shift+Equal", cmd | shift, Code::Equal),
            ("Ctrl+=", "Ctrl+Equal", ctrl, Code::Equal),
            ("Ctrl+Shift+?", "Ctrl+Shift+Slash", ctrl | shift, Code::Slash),
            ("Ctrl+Shift+!", "Ctrl+Shift+1", ctrl | shift, Code::Digit1),
            ("Cmd+Shift+_", "Cmd+Shift+Minus", cmd | shift, Code::Minus),
            ("Ctrl+Shift+:", "Ctrl+Shift+Semicolon", ctrl | shift, Code::Semicolon),
            ("Ctrl+Alt+;", "Ctrl+Alt+Semicolon", ctrl | alt, Code::Semicolon),
            ("Cmd+7", "Cmd+7", cmd, Code::Digit7),
            ("Cmd+F5", "Cmd+F5", cmd, Code::F5),
            ("Ctrl+F13", "Ctrl+F13", ctrl, Code::F13),
            ("Ctrl+Alt+Space", "Ctrl+Alt+Space", ctrl | alt, Code::Space),
            ("Cmd+ArrowUp", "Cmd+ArrowUp", cmd, Code::ArrowUp),
            // What an earlier version wrote for the keys named by their characters, and the names now.
            ("Cmd+Alt+-", "Cmd+Alt+Minus", cmd | alt, Code::Minus),
            ("Cmd+Alt+/", "Cmd+Alt+Slash", cmd | alt, Code::Slash),
            ("Cmd+Alt++", "Cmd+Alt+NumpadAdd", cmd | alt, Code::NumpadAdd),
            ("Cmd+Alt+*", "Cmd+Alt+NumpadMultiply", cmd | alt, Code::NumpadMultiply),
            ("Ctrl+Shift+*", "Ctrl+Shift+8", ctrl | shift, Code::Digit8),
            ("Cmd+Alt+numpadadd", "Cmd+Alt+NumpadAdd", cmd | alt, Code::NumpadAdd),
            ("Alt+Return", "Alt+Enter", alt, Code::Enter),
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

    /// Every name the app writes for a physical key is a key `global-hotkey` parses, and the one it names.
    #[test]
    fn the_names_of_physical_keys_register_as_those_keys() {
        use global_hotkey::hotkey::{Code, HotKey};
        for (name, code, cap) in [
            ("Minus", Code::Minus, "-"),
            ("Equal", Code::Equal, "="),
            ("Comma", Code::Comma, ","),
            ("Period", Code::Period, "."),
            ("Slash", Code::Slash, "/"),
            ("Backslash", Code::Backslash, "\\"),
            ("Semicolon", Code::Semicolon, ";"),
            ("Quote", Code::Quote, "'"),
            ("BracketLeft", Code::BracketLeft, "["),
            ("BracketRight", Code::BracketRight, "]"),
            ("Backquote", Code::Backquote, "`"),
            ("NumpadAdd", Code::NumpadAdd, "Num +"),
            ("NumpadSubtract", Code::NumpadSubtract, "Num -"),
            ("NumpadMultiply", Code::NumpadMultiply, "Num *"),
            ("NumpadDivide", Code::NumpadDivide, "Num /"),
            ("NumpadDecimal", Code::NumpadDecimal, "Num ."),
            ("NumpadEqual", Code::NumpadEqual, "Num ="),
            ("NumpadEnter", Code::NumpadEnter, "Num Enter"),
            ("Numpad0", Code::Numpad0, "Num 0"),
            ("Numpad5", Code::Numpad5, "Num 5"),
            ("Numpad9", Code::Numpad9, "Num 9"),
        ] {
            let kept = normalize_shortcut(&format!("Cmd+Alt+{name}")).unwrap();
            assert_eq!(kept, format!("Cmd+Alt+{name}"), "{name} is kept as it is");
            assert_eq!(
                normalize_shortcut(&format!("cmd+alt+{}", name.to_lowercase())).unwrap(),
                kept,
                "{name}: any case"
            );
            let hotkey: HotKey = kept.parse().unwrap_or_else(|e| panic!("{kept}: {e}"));
            assert_eq!(hotkey.key, code, "{name}");
            assert_eq!(keycap(name).as_deref(), Some(cap), "{name}");
        }
        // The ISO key is a key the app names, but `global-hotkey` does not know it on any system: it is refused as unsupported
        // (`shortcuts::key_reach`) and never registered as another key.
        assert!("Cmd+Alt+IntlBackslash".parse::<HotKey>().is_err());
        assert_eq!(normalize_shortcut("Cmd+Alt+IntlBackslash").as_deref(), Some("Cmd+Alt+IntlBackslash"));
        assert_eq!(keycap("IntlBackslash").as_deref(), Some("<"));
        assert_eq!(keycap("M"), None);
        assert_eq!(keycap("F5"), None);
    }

    #[test]
    fn a_shortcut_saved_in_the_old_form_is_the_same_as_in_the_new() {
        assert!(same_shortcut("Cmd+Alt+-", "Cmd+Alt+Minus"));
        assert!(same_shortcut("Ctrl+;", "ctrl+semicolon"));
        assert!(same_shortcut("Cmd+Alt++", "Cmd+Alt+NumpadAdd"));
        assert!(!same_shortcut("Cmd+Alt+-", "Cmd+Alt+NumpadSubtract"), "the main row's - is not the keypad's");
        assert!(!same_shortcut("Cmd+Alt+NumpadAdd", "Cmd+Alt+NumpadSubtract"));
        assert!(same_shortcut("Ctrl+Alt+M", "alt+control+m"));
    }

    #[test]
    fn what_is_not_a_shortcut() {
        for text in ["M", "Cmd", "Cmd+Shift", "Cmd+", "+M", "Cmd++M", "Cmd+M+K", "Hyper+M", "M+Cmd", "CmdOrCtrl+M"] {
            assert_eq!(normalize_shortcut(text), None, "{text}");
        }
    }

    #[test]
    fn the_mute_shortcut_reads_as_the_seam_says() {
        assert_eq!(
            serde_json::to_value(GlobalShortcut::new("", Registration::On, vec![], true, "en")).unwrap(),
            json!({"accelerator": null, "label": null, "state": "off", "alternatives": [], "voiceOver": false, "layoutBound": false, "managedBySystem": false})
        );
        assert_eq!(
            serde_json::to_value(GlobalShortcut::new("Ctrl+Alt+M", Registration::On, vec![], true, "en")).unwrap(),
            json!({"accelerator": "Ctrl+Alt+M", "label": "⌃⌥M", "state": "on", "alternatives": [], "voiceOver": false, "layoutBound": false, "managedBySystem": false})
        );
        let refused =
            GlobalShortcut::new("Cmd+Shift+M", Registration::Refused, vec!["Ctrl+Alt+Shift+M".into()], false, "en");
        assert_eq!(
            serde_json::to_value(refused).unwrap(),
            json!({"accelerator": "Cmd+Shift+M", "label": "Super+Shift+M", "state": "refused",
                   "alternatives": ["Ctrl+Alt+Shift+M"], "voiceOver": false, "layoutBound": false, "managedBySystem": false})
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
    fn the_floating_card_shows_unless_switched_off() {
        assert!(Settings::first_run(crate::shortcuts::DEFAULT_MUTE_MACOS).call_controls_enabled);
        let stored: Settings = serde_json::from_str("{}").unwrap();
        assert!(stored.call_controls_enabled, "a settings file from before the switch keeps the card on");
        let off: Settings = serde_json::from_str(r#"{"callControlsEnabled":false}"#).unwrap();
        assert!(!off.call_controls_enabled);
    }

    #[test]
    fn round_trip_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("sidevoice-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), None, "first run: nothing stored");

        let s = Settings {
            mute_shortcut: "Alt+M".into(),
            hold_to_talk_shortcut: "Fn:hold".into(),
            one_turn_shortcut: "RightControl:double".into(),
            shortcuts_migrated: true,
            headset_press: crate::shortcuts::HeadsetPress::OneTurn,
            headset_double_press: crate::shortcuts::HeadsetPress::Skip,
            input_monitoring_asked: true,
            call_controls_always_shown: true,
            call_controls_enabled: false,
            update_mode: crate::updater::Mode::Manual,
            call_controls_corner: crate::call_controls::Corner::BottomLeft,
            debug: crate::debug_options::DebugOptions {
                inspectable: true,
                compositing_borders: false,
                repaint_counters: true,
            },
            media_duck: crate::media_duck::Mode::Pause,
            media_duck_level: crate::media_duck::Level::Mute,
        };
        save(&dir, &s).unwrap();
        assert_eq!(load(&dir), Some(s));

        fs::write(dir.join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(load(&dir), None, "corrupt file: start as a first run rather than guess");

        fs::write(dir.join(FILE_NAME), "{}").unwrap();
        let bare = load(&dir).unwrap();
        assert_eq!(
            bare.mute_shortcut, "Ctrl+Alt+M",
            "a file from before the shortcut could be set holds what every system had"
        );
        assert!(!bare.shortcuts_migrated && bare.hold_to_talk_shortcut.is_empty() && bare.one_turn_shortcut.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_old_mute_default_moves_once_where_the_default_changed() {
        use crate::shortcuts::{DEFAULT_MUTE_MACOS as MAC, DEFAULT_MUTE_OTHER as OTHER};
        assert_eq!(Settings::first_run(OTHER).mute_shortcut, OTHER);
        assert!(Settings::first_run(OTHER).shortcuts_migrated, "a new install has nothing to move");
        let old = |mute: &str| Settings { mute_shortcut: mute.into(), ..serde_json::from_str("{}").unwrap() };
        let mut stored = old("Ctrl+Alt+M");
        assert!(stored.migrate_shortcuts(OTHER));
        assert_eq!((stored.mute_shortcut.as_str(), stored.shortcuts_migrated), (OTHER, true));
        // Chosen again afterwards: kept, the move does not run twice.
        stored.mute_shortcut = "Ctrl+Alt+M".into();
        assert!(!stored.migrate_shortcuts(OTHER));
        assert_eq!(stored.mute_shortcut, "Ctrl+Alt+M");
        let mut chosen = old("Cmd+Shift+M");
        assert!(chosen.migrate_shortcuts(OTHER));
        assert_eq!(chosen.mute_shortcut, "Cmd+Shift+M", "an explicit choice stays");
        let mut off = old("");
        off.migrate_shortcuts(OTHER);
        assert_eq!(off.mute_shortcut, "", "off stays off");
        let mut mac = old("Ctrl+Alt+M");
        assert!(mac.migrate_shortcuts(MAC));
        assert_eq!(mac.mute_shortcut, MAC, "macOS keeps ⌃⌥M, and is marked so");
    }
}
