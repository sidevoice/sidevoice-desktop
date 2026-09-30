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

/// A node on this machine, as the connector starts it (sidevoice-core's fixed default port).
pub const DEFAULT_NODE_URL: &str = "http://127.0.0.1:8768/";

/// The origin Tauri serves the app's own pages from: what a node sees as the bundled page's `Origin`.
#[cfg(windows)]
pub const APP_ORIGIN: &str = "http://tauri.localhost";
#[cfg(not(windows))]
pub const APP_ORIGIN: &str = "tauri://localhost";

/// What the address points at (docs/TARGETS.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    /// A hosted room (the old one or the split's rendezvous): the window opens the room's own page, so its
    /// sign-in, its pairing panel and its version of the interface are the room's.
    #[default]
    Room,
    /// A machine's core directly: the window opens the interface bundled in the app, pointed at it through
    /// `window.__SIDEVOICE_TARGET__` (the node serves no page).
    Node,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Room or node; files from before nodes existed are rooms.
    #[serde(default)]
    pub kind: TargetKind,
    /// The room's or the node's address, normalised with [`normalize_room_url`]; never stored raw.
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
        Settings { kind: TargetKind::Room, room_url: DEFAULT_ROOM_URL.to_string(), mute_shortcut: default_shortcut() }
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

/// `https://voice.example.com` — the origin of an address.
pub fn origin_of(room_url: &str) -> Result<String, SettingsError> {
    let url = Url::parse(room_url).map_err(|e| SettingsError::Invalid(e.to_string()))?;
    Ok(url_origin(&url))
}

/// An URL's origin as a page on it reports it. The WHATWG origin of a custom scheme (`tauri://localhost`) is
/// opaque (`null`), but WebKit and WebView2 report the app's pages as `scheme://host`, and so do these.
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

/// The origin of the page the window shows: the room's own, or the app's for the bundled interface.
pub fn page_origin(settings: &Settings) -> Result<String, SettingsError> {
    match settings.kind {
        TargetKind::Room => origin_of(&settings.room_url),
        TargetKind::Node => Ok(APP_ORIGIN.to_string()),
    }
}

/// For a node: the script that tells the bundled interface where its target is (the web client contract,
/// rubasace/sidevoice docs/RENDEZVOUS.md). Only on the app's own pages; `None` for a room.
pub fn target_script(settings: &Settings) -> Option<String> {
    if settings.kind != TargetKind::Node {
        return None;
    }
    let target = serde_json::to_string(settings.room_url.trim_end_matches('/')).expect("a string serialises");
    let app = serde_json::to_string(APP_ORIGIN).expect("a string serialises");
    Some(format!(
        "if (location.protocol + '//' + location.host === {app}) {{ window.__SIDEVOICE_TARGET__ = {target}; }}"
    ))
}

/// `https://voice.example.com/*` — the remote-URL pattern of the capability that lets the room
/// page (and only it) report call state to the app.
pub fn remote_pattern(room_url: &str) -> Result<String, SettingsError> {
    Ok(format!("{}/*", origin_of(room_url)?))
}

/// Validates and normalises a whole settings object coming from the settings window.
pub fn validate(input: Settings) -> Result<Settings, SettingsError> {
    Ok(Settings {
        kind: input.kind,
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
    fn custom_scheme_origins_are_scheme_and_host() {
        assert_eq!(url_origin(&Url::parse("tauri://localhost/voice/index.html").unwrap()), "tauri://localhost");
        assert_eq!(url_origin(&Url::parse("http://tauri.localhost/voice/").unwrap()), "http://tauri.localhost");
        assert_eq!(
            url_origin(&Url::parse("https://voice.example.com:8443/a").unwrap()),
            "https://voice.example.com:8443"
        );
        assert_eq!(url_origin(&Url::parse("about:blank").unwrap()), "null");
    }

    #[test]
    fn a_room_is_its_own_page_a_node_is_the_app_s() {
        let room = Settings::default();
        assert_eq!(page_origin(&room).unwrap(), "https://room.example.invalid");
        assert_eq!(target_script(&room), None, "a room's own page talks to its own origin");

        let node =
            Settings { kind: TargetKind::Node, room_url: "http://127.0.0.1:8768/".into(), ..Settings::default() };
        assert_eq!(page_origin(&node).unwrap(), APP_ORIGIN);
        let script = target_script(&node).unwrap();
        assert!(script.contains(r#"window.__SIDEVOICE_TARGET__ = "http://127.0.0.1:8768";"#), "{script}");
        assert!(script.contains(&format!("=== \"{APP_ORIGIN}\"")), "only on the app's own pages: {script}");
    }

    #[test]
    fn kind_serialises_lowercase() {
        let node = Settings { kind: TargetKind::Node, ..Settings::default() };
        assert!(serde_json::to_string(&node).unwrap().contains(r#""kind":"node""#));
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

        let s = Settings {
            kind: TargetKind::Node,
            room_url: "http://127.0.0.1:8768/".into(),
            mute_shortcut: "Alt+M".into(),
        };
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
                kind: TargetKind::Room,
                room_url: "https://voice.example.com/".into(),
                mute_shortcut: DEFAULT_MUTE_SHORTCUT.into()
            }),
            "older files without a kind or a shortcut are a room with the default shortcut"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
