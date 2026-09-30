//! The app's side of the desktop bridge (docs/BRIDGE.md).
//!
//! The room page reports a [`CallSnapshot`] through the `bridge_state` command; the app drives the
//! page with a [`Command`], evaluated as a call to `window.__sidevoiceDesktop.run(...)`. Pure logic
//! here; the Tauri wiring lives in `lib.rs`.

use serde::{Deserialize, Serialize};

/// The bridge script, with its room-origin placeholder still in place.
pub const SCRIPT_TEMPLATE: &str = include_str!("../../../bridge/desktop-bridge.js");
const ORIGIN_PLACEHOLDER: &str = "\"__SIDEVOICE_ROOM_ORIGIN__\"";

/// The script injected into the main window, bound to one room origin.
pub fn script_for_origin(origin: &str) -> String {
    let literal = serde_json::to_string(origin).expect("a string serialises");
    SCRIPT_TEMPLATE.replacen(ORIGIN_PLACEHOLDER, &literal, 1)
}

/// What the tray shows. Mirrors the web UI's own `call` and `mic` views.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CallSnapshot {
    pub version: u32,
    /// The room's controller is up and the bridge is attached to it.
    pub ready: bool,
    /// In a call, or joining one.
    pub joined: bool,
    /// Reconnecting or switching session.
    pub busy: bool,
    pub mic_enabled: bool,
    /// The web UI's mute button is disabled (in a call with no conversation selected).
    pub mic_disabled: bool,
    /// The conversation being looked at.
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ToggleMute,
    HangUp,
}

impl Command {
    pub fn name(self) -> &'static str {
        match self {
            Command::ToggleMute => "toggle-mute",
            Command::HangUp => "hang-up",
        }
    }

    /// The JavaScript the app evaluates in the room page. A page without the bridge ignores it.
    pub fn script(self) -> String {
        format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.run({:?});", self.name())
    }
}

/// Which of the three tray icons to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIcon {
    Idle,
    Live,
    Muted,
}

/// Everything the tray displays, derived from one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub icon: TrayIcon,
    pub status: String,
    pub mute_label: &'static str,
    pub mute_enabled: bool,
    pub hang_up_enabled: bool,
}

pub fn tray_view(s: &CallSnapshot) -> TrayView {
    if !s.ready {
        return TrayView {
            icon: TrayIcon::Idle,
            status: "Sidevoice · sala sin cargar".into(),
            mute_label: "Silenciar micrófono",
            mute_enabled: false,
            hang_up_enabled: false,
        };
    }
    let mute_label = if s.mic_enabled { "Silenciar micrófono" } else { "Activar micrófono" };
    let (icon, status) = match (s.joined, s.mic_enabled) {
        (false, _) => (TrayIcon::Idle, "Sin llamada".to_string()),
        (true, _) if s.busy => (TrayIcon::Live, "Reconectando…".to_string()),
        (true, true) => (TrayIcon::Live, with_title("En llamada", &s.title)),
        (true, false) => (TrayIcon::Muted, with_title("En llamada · silenciado", &s.title)),
    };
    TrayView { icon, status, mute_label, mute_enabled: !s.mic_disabled, hang_up_enabled: s.joined }
}

fn with_title(status: &str, title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        return status.to_string();
    }
    // Menu items are one line; keep a long conversation title from widening the menu.
    let short: String = title.chars().take(40).collect();
    let ellipsis = if title.chars().count() > 40 { "…" } else { "" };
    format!("{status} · {short}{ellipsis}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(mic_enabled: bool) -> CallSnapshot {
        CallSnapshot {
            version: 1,
            ready: true,
            joined: true,
            mic_enabled,
            title: "Claude".into(),
            ..Default::default()
        }
    }

    #[test]
    fn script_binds_the_origin_as_a_js_string() {
        let s = script_for_origin("https://voice.example.com");
        assert!(s.contains(r#"factory(window, "https://voice.example.com")"#));
        assert!(!s.contains("__SIDEVOICE_ROOM_ORIGIN__"));
        // A hostile value cannot break out of the string literal.
        let evil = script_for_origin("\"); alert(1); (\"");
        assert!(evil.contains(r#""\"); alert(1); (\"""#));
    }

    #[test]
    fn commands_are_fixed_strings() {
        assert_eq!(
            Command::ToggleMute.script(),
            r#"window.__sidevoiceDesktop && window.__sidevoiceDesktop.run("toggle-mute");"#
        );
        assert_eq!(Command::HangUp.name(), "hang-up");
    }

    #[test]
    fn snapshot_parses_what_the_script_sends() {
        let json = r#"{"version":1,"ready":true,"joined":true,"busy":false,"micEnabled":false,"micDisabled":false,"title":"x","extra":1}"#;
        let s: CallSnapshot = serde_json::from_str(json).unwrap();
        assert!(s.ready && s.joined && !s.mic_enabled);
        let partial: CallSnapshot = serde_json::from_str(r#"{"ready":false}"#).unwrap();
        assert!(!partial.ready);
    }

    #[test]
    fn tray_states() {
        let not_ready = tray_view(&CallSnapshot::default());
        assert_eq!(not_ready.icon, TrayIcon::Idle);
        assert!(!not_ready.mute_enabled && !not_ready.hang_up_enabled);

        let idle = tray_view(&CallSnapshot { ready: true, mic_enabled: true, ..Default::default() });
        assert_eq!((idle.icon, idle.status.as_str()), (TrayIcon::Idle, "Sin llamada"));
        assert!(idle.mute_enabled, "mute before joining sets the preference, as in the web UI");
        assert!(!idle.hang_up_enabled);

        let on = tray_view(&live(true));
        assert_eq!(
            (on.icon, on.status.as_str(), on.mute_label),
            (TrayIcon::Live, "En llamada · Claude", "Silenciar micrófono")
        );
        assert!(on.hang_up_enabled);

        let muted = tray_view(&live(false));
        assert_eq!((muted.icon, muted.mute_label), (TrayIcon::Muted, "Activar micrófono"));

        let busy = tray_view(&CallSnapshot { busy: true, ..live(true) });
        assert_eq!(busy.status, "Reconectando…");

        let no_conversation = tray_view(&CallSnapshot { mic_disabled: true, ..live(true) });
        assert!(!no_conversation.mute_enabled);
    }

    #[test]
    fn long_titles_are_cut() {
        let s = tray_view(&CallSnapshot { title: "a".repeat(100), ..live(true) });
        assert!(s.status.ends_with('…'));
        assert!(s.status.chars().count() < 70);
    }
}
