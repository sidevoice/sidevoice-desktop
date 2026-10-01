//! What this device remembers: where the bundled interface looks first (its target), and the mute shortcut.
//!
//! The window always shows the web interface bundled in the app (docs/TARGETS.md). Which machine it talks to,
//! and how to reach it, comes from pairing (the code the node issues carries the node's addresses); the target
//! is only where the interface looks first — a node, or a rendezvous room — and may be empty.
//!
//! Pure logic plus a JSON file in the app's config directory. No Tauri types here, so it is unit-tested on any
//! machine.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::Path;
use url::Url;

/// Global mute toggle. ⌘⇧M on macOS, Ctrl+Shift+M elsewhere; editable in settings.
pub const DEFAULT_MUTE_SHORTCUT: &str = "CmdOrCtrl+Shift+M";

pub const FILE_NAME: &str = "settings.json";

/// The origin Tauri serves the app's own pages from: what a node sees as the interface's `Origin`.
#[cfg(windows)]
pub const APP_ORIGIN: &str = "http://tauri.localhost";
#[cfg(not(windows))]
pub const APP_ORIGIN: &str = "tauri://localhost";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// A node's or a rendezvous room's address, normalised with [`normalize_target`]; empty for none.
    /// (Files from before 2026-09-30 carried `roomUrl`, a room whose page the window loaded; it is ignored.)
    #[serde(default)]
    pub target: String,
    /// A Tauri accelerator string; empty disables the global shortcut.
    #[serde(default = "default_shortcut")]
    pub mute_shortcut: String,
}

fn default_shortcut() -> String {
    DEFAULT_MUTE_SHORTCUT.to_string()
}

impl Settings {
    /// First run: no target (pairing will say where the machine is), the default shortcut.
    pub fn first_run() -> Self {
        Settings { target: String::new(), mute_shortcut: default_shortcut() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    Invalid(String),
    /// A secure page may only call https, or http on this machine (mixed content otherwise).
    Insecure,
    Credentials,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Invalid(why) => write!(f, "Esa dirección no es válida: {why}."),
            SettingsError::Insecure => write!(
                f,
                "La dirección tiene que ir por https:// (http:// solo vale para este ordenador: localhost o 127.0.0.1)."
            ),
            SettingsError::Credentials => write!(f, "La dirección no puede llevar usuario ni contraseña."),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Turns what the person typed into the target the interface is told.
///
/// - empty → no target
/// - `node.example.com` → `https://node.example.com` (https is assumed, never http)
/// - only `https`, or `http` on loopback (a node on this machine), is accepted
/// - no trailing slash, no fragment; a path is kept (a room may live under one)
pub fn normalize_target(input: &str) -> Result<String, SettingsError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
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
    let text = url.to_string();
    Ok(match text.strip_suffix('/') {
        Some(stripped) if url.query().is_none() => stripped.to_string(),
        _ => text,
    })
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
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

/// The script that tells the bundled interface its target (the web client contract, rubasace/sidevoice
/// docs/RENDEZVOUS.md and docs/DEVICE_PAIRING.md). Only on the app's own pages; `None` without a target.
pub fn target_script(settings: &Settings) -> Option<String> {
    if settings.target.is_empty() {
        return None;
    }
    let target = serde_json::to_string(&settings.target).expect("a string serialises");
    let app = serde_json::to_string(APP_ORIGIN).expect("a string serialises");
    Some(format!(
        "if (location.protocol + '//' + location.host === {app}) {{ window.__SIDEVOICE_TARGET__ = {target}; }}"
    ))
}

/// Validates and normalises a whole settings object coming from the settings window.
pub fn validate(input: Settings) -> Result<Settings, SettingsError> {
    Ok(Settings { target: normalize_target(&input.target)?, mute_shortcut: input.mute_shortcut.trim().to_string() })
}

/// `None` when there is no file yet (first run) or it cannot be read as valid settings.
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
    fn empty_is_no_target() {
        assert_eq!(normalize_target("   ").unwrap(), "");
        assert_eq!(target_script(&Settings::first_run()), None);
    }

    #[test]
    fn bare_host_becomes_https_without_trailing_slash() {
        assert_eq!(normalize_target("node.example.com").unwrap(), "https://node.example.com");
        assert_eq!(normalize_target("  https://room.example.com/  ").unwrap(), "https://room.example.com");
        assert_eq!(normalize_target("https://room.example.com/sv/#f").unwrap(), "https://room.example.com/sv");
    }

    #[test]
    fn plain_http_only_on_loopback() {
        assert_eq!(normalize_target("http://127.0.0.1:8768").unwrap(), "http://127.0.0.1:8768");
        assert_eq!(normalize_target("http://localhost:8769/").unwrap(), "http://localhost:8769");
        assert_eq!(normalize_target("http://[::1]:8768").unwrap(), "http://[::1]:8768");
        assert_eq!(normalize_target("http://node.example.com"), Err(SettingsError::Insecure));
        assert_eq!(normalize_target("http://192.168.1.10:8768"), Err(SettingsError::Insecure));
    }

    #[test]
    fn rejects_the_rest() {
        assert!(matches!(normalize_target("file:///etc/passwd"), Err(SettingsError::Invalid(_))));
        assert!(matches!(normalize_target("javascript://alert(1)"), Err(SettingsError::Invalid(_))));
        assert!(matches!(normalize_target("https://"), Err(SettingsError::Invalid(_))));
        assert_eq!(normalize_target("https://me:pw@node.example.com"), Err(SettingsError::Credentials));
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
    fn the_target_script_binds_to_the_app_s_pages_and_escapes() {
        let s = Settings { target: "http://127.0.0.1:8768".into(), ..Settings::first_run() };
        let script = target_script(&s).unwrap();
        assert!(script.contains(r#"window.__SIDEVOICE_TARGET__ = "http://127.0.0.1:8768";"#), "{script}");
        assert!(script.contains(&format!("=== \"{APP_ORIGIN}\"")), "{script}");
        let evil = Settings { target: "\"; alert(1); \"".into(), ..Settings::first_run() };
        assert!(target_script(&evil).unwrap().contains(r#""\"; alert(1); \"""#));
    }

    #[test]
    fn round_trip_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("sidevoice-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), None, "first run: nothing stored");

        let s = Settings { target: "https://room.example.com".into(), mute_shortcut: "Alt+M".into() };
        save(&dir, &s).unwrap();
        assert_eq!(load(&dir), Some(s));

        fs::write(dir.join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(load(&dir), None, "corrupt file: start as a first run rather than guess");

        fs::write(dir.join(FILE_NAME), r#"{"target":"http://evil.example.com"}"#).unwrap();
        assert_eq!(load(&dir), None, "a stored insecure address is not trusted either");

        fs::write(
            dir.join(FILE_NAME),
            r#"{"kind":"room","roomUrl":"https://room.example.com/","muteShortcut":"Alt+M"}"#,
        )
        .unwrap();
        assert_eq!(
            load(&dir),
            Some(Settings { target: String::new(), mute_shortcut: "Alt+M".into() }),
            "a file from the room-page days keeps its shortcut and forgets the room"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
