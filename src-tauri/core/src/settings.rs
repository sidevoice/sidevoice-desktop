//! What this device remembers: which room (or node) the window opens, and the mute shortcut.
//!
//! Pure logic plus a JSON file in the app's config directory. No Tauri types here, so it is
//! unit-tested on any machine.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::Path;
use url::Url;

/// The operator's room. Source: the room this deployment's connector is paired with
/// (`url` in `~/.sidevoice/credentials.json` on the operator's pod), 2026-09-30.
pub const DEFAULT_ROOM_URL: &str = "https://room.example.invalid/";

/// Global mute toggle. ⌘⇧M on macOS, Ctrl+Shift+M elsewhere; editable in settings.
pub const DEFAULT_MUTE_SHORTCUT: &str = "CmdOrCtrl+Shift+M";

pub const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Normalised with [`normalize_room_url`]; never stored raw.
    pub room_url: String,
    /// A Tauri accelerator string; empty disables the global shortcut.
    #[serde(default = "default_shortcut")]
    pub mute_shortcut: String,
}

fn default_shortcut() -> String {
    DEFAULT_MUTE_SHORTCUT.to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Settings { room_url: DEFAULT_ROOM_URL.to_string(), mute_shortcut: default_shortcut() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    Empty,
    Invalid(String),
    /// getUserMedia only exists in a secure context: https, or http on this machine.
    Insecure,
    Credentials,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Empty => write!(f, "Escribe la dirección de tu sala."),
            SettingsError::Invalid(why) => write!(f, "Esa dirección no es válida: {why}."),
            SettingsError::Insecure => write!(
                f,
                "La sala tiene que ir por https:// (http:// solo vale para localhost): sin eso el micrófono no funciona."
            ),
            SettingsError::Credentials => write!(f, "La dirección no puede llevar usuario ni contraseña."),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Turns what the person typed into the URL the window loads.
///
/// - `voice.example.com` → `https://voice.example.com/` (https is assumed, never http)
/// - only `https`, or `http` on loopback (a node on this machine), is accepted
/// - the fragment is dropped; path and query are kept (a node may live under a path)
pub fn normalize_room_url(input: &str) -> Result<String, SettingsError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(SettingsError::Empty);
    }
    let candidate = if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
    let mut url = Url::parse(&candidate).map_err(|e| SettingsError::Invalid(e.to_string()))?;
    let host = url.host_str().unwrap_or("").to_string();
    if host.is_empty() {
        return Err(SettingsError::Invalid("falta el nombre del servidor".into()));
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        "http" => return Err(SettingsError::Insecure),
        other => return Err(SettingsError::Invalid(format!("esquema {other}:// no soportado"))),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SettingsError::Credentials);
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// `https://voice.example.com` — what the bridge script compares `location.origin` with.
pub fn origin_of(room_url: &str) -> Result<String, SettingsError> {
    let url = Url::parse(room_url).map_err(|e| SettingsError::Invalid(e.to_string()))?;
    Ok(url.origin().ascii_serialization())
}

/// `https://voice.example.com/*` — the remote-URL pattern of the capability that lets the room
/// page (and only it) report call state to the app.
pub fn remote_pattern(room_url: &str) -> Result<String, SettingsError> {
    Ok(format!("{}/*", origin_of(room_url)?))
}

/// Validates and normalises a whole settings object coming from the settings window.
pub fn validate(input: Settings) -> Result<Settings, SettingsError> {
    Ok(Settings {
        room_url: normalize_room_url(&input.room_url)?,
        mute_shortcut: input.mute_shortcut.trim().to_string(),
    })
}

/// `None` when there is no file yet (first run) or it cannot be read as valid settings; the
/// caller then shows the settings window instead of guessing.
pub fn load(dir: &Path) -> Option<Settings> {
    let raw = fs::read_to_string(dir.join(FILE_NAME)).ok()?;
    let parsed: Settings = serde_json::from_str(&raw).ok()?;
    validate(parsed).ok()
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

    #[test]
    fn bare_host_becomes_https() {
        assert_eq!(normalize_room_url("voice.example.com").unwrap(), "https://voice.example.com/");
        assert_eq!(normalize_room_url("  https://voice.example.com  ").unwrap(), "https://voice.example.com/");
    }

    #[test]
    fn keeps_path_and_query_drops_fragment() {
        assert_eq!(
            normalize_room_url("https://node.example.com/sidevoice/?room=a#x").unwrap(),
            "https://node.example.com/sidevoice/?room=a"
        );
    }

    #[test]
    fn plain_http_only_on_loopback() {
        assert_eq!(normalize_room_url("http://localhost:8000").unwrap(), "http://localhost:8000/");
        assert_eq!(normalize_room_url("http://127.0.0.1:8000/").unwrap(), "http://127.0.0.1:8000/");
        assert_eq!(normalize_room_url("http://[::1]:8000/").unwrap(), "http://[::1]:8000/");
        assert_eq!(normalize_room_url("http://voice.example.com"), Err(SettingsError::Insecure));
        assert_eq!(normalize_room_url("http://192.168.1.10:8000"), Err(SettingsError::Insecure));
    }

    #[test]
    fn rejects_the_rest() {
        assert_eq!(normalize_room_url("   "), Err(SettingsError::Empty));
        assert!(matches!(normalize_room_url("file:///etc/passwd"), Err(SettingsError::Invalid(_))));
        assert!(matches!(normalize_room_url("javascript://alert(1)"), Err(SettingsError::Invalid(_))));
        assert!(matches!(normalize_room_url("https://"), Err(SettingsError::Invalid(_))));
        assert_eq!(normalize_room_url("https://me:pw@voice.example.com"), Err(SettingsError::Credentials));
    }

    #[test]
    fn origin_and_pattern() {
        assert_eq!(origin_of("https://voice.example.com/a/b?c").unwrap(), "https://voice.example.com");
        assert_eq!(origin_of("http://localhost:8000/").unwrap(), "http://localhost:8000");
        assert_eq!(remote_pattern("https://voice.example.com:8443/x").unwrap(), "https://voice.example.com:8443/*");
    }

    #[test]
    fn default_is_valid() {
        let d = Settings::default();
        assert_eq!(validate(d.clone()).unwrap(), d);
    }

    #[test]
    fn round_trip_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("sidevoice-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), None, "first run: nothing stored");

        let s = Settings { room_url: "https://voice.example.com/".into(), mute_shortcut: "Alt+M".into() };
        save(&dir, &s).unwrap();
        assert_eq!(load(&dir), Some(s));

        fs::write(dir.join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(load(&dir), None, "corrupt file: ask again rather than guess");

        fs::write(dir.join(FILE_NAME), r#"{"roomUrl":"http://evil.example.com"}"#).unwrap();
        assert_eq!(load(&dir), None, "a stored insecure URL is not trusted either");

        fs::write(dir.join(FILE_NAME), r#"{"roomUrl":"voice.example.com"}"#).unwrap();
        assert_eq!(
            load(&dir),
            Some(Settings {
                room_url: "https://voice.example.com/".into(),
                mute_shortcut: DEFAULT_MUTE_SHORTCUT.into()
            }),
            "older files without a shortcut get the default"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
