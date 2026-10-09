//! The app's side of the desktop bridge (docs/BRIDGE.md).
//!
//! The room page reports a [`CallSnapshot`] through the `bridge_state` command; the app drives the
//! page with a [`Command`], evaluated as a call to `window.__sidevoiceDesktop.run(...)`. Pure logic
//! here; the Tauri wiring lives in `lib.rs`.

use serde::{Deserialize, Serialize};

/// The bridge script, with its room-origin placeholder still in place.
pub const SCRIPT_TEMPLATE: &str = include_str!("../../../bridge/desktop-bridge.js");
const ORIGIN_PLACEHOLDER: &str = "\"__SIDEVOICE_ROOM_ORIGIN__\"";
const MEDIA_KEYS_PLACEHOLDER: &str = "\"__SIDEVOICE_MEDIA_KEYS__\"";
const LOCAL_HOST_PLACEHOLDER: &str = "\"__SIDEVOICE_LOCAL_HOST__\"";
const VOICE_PLACEHOLDER: &str = "\"__SIDEVOICE_VOICE__\"";

/// Who answers the headset's buttons and media keys in a call: `native` where the app does (macOS,
/// `src/headset.rs`), so the page does not answer them too; `None` elsewhere (the page's Media Session does).
pub fn media_keys() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("native")
    } else {
        None
    }
}

/// Whether the app offers this computer's own core, the local host (`window.__sidevoiceDesktop.host.localHost`,
/// docs/LOCAL_HOST.md): on macOS only, the beta's app platform (design O2). Elsewhere the app is a remote client.
pub fn local_host_offered() -> bool {
    cfg!(target_os = "macos")
}

/// Whether the app runs the voice call itself (`window.__sidevoiceDesktop.host.voice`, docs/BRIDGE.md → "The voice
/// call"): on macOS only, where its echo canceller builds and the beta ships. Elsewhere the page runs the call, and the
/// room window grants it the microphone (`media::decide`).
pub fn voice_offered() -> bool {
    cfg!(target_os = "macos")
}

/// The call controls card's bridge script, with its origin placeholder still in place.
pub const CALL_CONTROLS_SCRIPT_TEMPLATE: &str = include_str!("../../../bridge/call-controls-bridge.js");
const APP_ORIGIN_PLACEHOLDER: &str = "\"__SIDEVOICE_APP_ORIGIN__\"";

/// The script injected into the call controls card's window, bound to the app's own origin.
pub fn call_controls_script(origin: &str) -> String {
    let literal = serde_json::to_string(origin).expect("a string serialises");
    CALL_CONTROLS_SCRIPT_TEMPLATE.replacen(APP_ORIGIN_PLACEHOLDER, &literal, 1)
}

/// What the app pushes to the card: any subset of `{call, level, pointerInside, alwaysExpanded, muteShortcut}`.
pub fn call_controls_push(patch: &serde_json::Value) -> String {
    format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.receive({patch});")
}

/// The script injected into the main window, bound to one page origin.
pub fn script_for_origin(origin: &str) -> String {
    let literal = serde_json::to_string(origin).expect("a string serialises");
    let keys = serde_json::to_string(&media_keys()).expect("serialises");
    let flag = |offered: bool| if offered { "true" } else { "false" };
    SCRIPT_TEMPLATE
        .replacen(ORIGIN_PLACEHOLDER, &literal, 1)
        .replacen(MEDIA_KEYS_PLACEHOLDER, &keys, 1)
        .replacen(LOCAL_HOST_PLACEHOLDER, flag(local_host_offered()), 1)
        .replacen(VOICE_PLACEHOLDER, flag(voice_offered()), 1)
}

/// The bridge's version: the room page sends it in every snapshot (bridge/desktop-bridge.js).
pub const VERSION: u32 = 2;

/// What the agent is doing, as the room's view model says (its `session`): the card's avatar shows it, and only it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    #[default]
    Idle,
    Working,
    Speaking,
}

/// The call as the room page reports it: what the tray and the headset need, and what the call controls card shows.
/// Mirrors the web UI's own views; `participants` and `devices` are relayed to the card as the room wrote them (the
/// web UI owns their shape: `ParticipantView`, `AudioDevices`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    pub agent: AgentState,
    /// The person is speaking (a turn is open).
    pub you_talking: bool,
    /// Something is playing that "skip" stops.
    pub can_skip: bool,
    /// When the call was joined, in milliseconds since the Unix epoch; `None` outside a call.
    pub since: Option<f64>,
    pub participants: Vec<serde_json::Value>,
    pub devices: Option<serde_json::Value>,
}

/// What the app asks the room page to do (tray, global shortcut, headset, call controls card).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Command {
    ToggleMute,
    HangUp,
    SkipReply,
    #[serde(rename_all = "camelCase")]
    SelectParticipant {
        thread_id: String,
    },
    SelectAudioDevice {
        kind: DeviceKind,
        id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Input,
    Output,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::ToggleMute => "toggle-mute",
            Command::HangUp => "hang-up",
            Command::SkipReply => "skip-reply",
            Command::SelectParticipant { .. } => "select-participant",
            Command::SelectAudioDevice { .. } => "select-audio-device",
        }
    }

    /// The JavaScript the app evaluates in the room page. A page without the bridge ignores it. The command travels
    /// as JSON, so no value it carries can break out of the call.
    pub fn script(&self) -> String {
        let json = serde_json::to_string(self).expect("a command serialises");
        format!("window.__sidevoiceDesktop && window.__sidevoiceDesktop.run({json});")
    }
}

/// Which of the three tray icons to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIcon {
    Idle,
    Live,
    Muted,
}

/// Everything the tray displays, derived from one snapshot, in the app's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub icon: TrayIcon,
    pub status: String,
    pub mute_label: &'static str,
    pub mute_enabled: bool,
    pub hang_up_enabled: bool,
}

pub fn tray_view(s: &CallSnapshot, language: &str) -> TrayView {
    let t = |key: &str| crate::i18n::t(language, key);
    if !s.ready {
        return TrayView {
            icon: TrayIcon::Idle,
            status: t("tray.not_loaded").into(),
            mute_label: t("tray.mute"),
            mute_enabled: false,
            hang_up_enabled: false,
        };
    }
    let mute_label = if s.mic_enabled { t("tray.mute") } else { t("tray.unmute") };
    let (icon, status) = match (s.joined, s.mic_enabled) {
        (false, _) => (TrayIcon::Idle, t("tray.no_call").to_string()),
        (true, _) if s.busy => (TrayIcon::Live, t("tray.reconnecting").to_string()),
        (true, true) => (TrayIcon::Live, with_title(t("tray.in_call"), &s.title)),
        (true, false) => (TrayIcon::Muted, with_title(t("tray.in_call_muted"), &s.title)),
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
            version: VERSION,
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
        assert!(!s.contains("__SIDEVOICE_ROOM_ORIGIN__"));
        assert!(!s.contains("__SIDEVOICE_MEDIA_KEYS__"));
        assert!(!s.contains("__SIDEVOICE_LOCAL_HOST__"));
        assert!(!s.contains("__SIDEVOICE_VOICE__"));
        let keys = if cfg!(target_os = "macos") { r#""native", true, true)"# } else { "null, false, false)" };
        assert!(s.contains(&format!(r#"factory(window, "https://voice.example.com", {keys}"#)), "{keys}");
        // A hostile value cannot break out of the string literal.
        let evil = script_for_origin("\"); alert(1); (\"");
        assert!(evil.contains(r#""\"); alert(1); (\"""#));
    }

    #[test]
    fn the_card_s_script_binds_the_app_s_origin() {
        let s = call_controls_script("tauri://localhost");
        assert!(!s.contains("__SIDEVOICE_APP_ORIGIN__"));
        assert!(s.contains(r#"factory(window, "tauri://localhost")"#));
        let push = call_controls_push(&serde_json::json!({"level": 3, "title": "\"); alert(1); (\""}));
        assert!(push.contains(r#"receive({"level":3,"title":"\"); alert(1); (\""});"#), "{push}");
    }

    #[test]
    fn commands_travel_as_json() {
        assert_eq!(
            Command::ToggleMute.script(),
            r#"window.__sidevoiceDesktop && window.__sidevoiceDesktop.run({"command":"toggle-mute"});"#
        );
        assert_eq!(Command::HangUp.name(), "hang-up");
        let pick = Command::SelectParticipant { thread_id: "t\"); alert(1); (\"".into() };
        assert_eq!(pick.name(), "select-participant");
        assert!(
            pick.script().contains(r#"{"command":"select-participant","threadId":"t\"); alert(1); (\""}"#),
            "{}",
            pick.script()
        );
        let device = Command::SelectAudioDevice { kind: DeviceKind::Input, id: "mbp".into() };
        assert!(device.script().contains(r#"{"command":"select-audio-device","kind":"input","id":"mbp"}"#));
    }

    #[test]
    fn the_card_s_commands_parse_and_nothing_else_does() {
        let parse = |json: &str| serde_json::from_str::<Command>(json);
        assert_eq!(parse(r#"{"command":"skip-reply"}"#).unwrap(), Command::SkipReply);
        assert_eq!(
            parse(r#"{"command":"select-participant","threadId":"abc"}"#).unwrap(),
            Command::SelectParticipant { thread_id: "abc".into() }
        );
        assert!(parse(r#"{"command":"select-audio-device","kind":"camera","id":"x"}"#).is_err());
        assert!(parse(r#"{"command":"close-participant","threadId":"abc"}"#).is_err());
        assert!(parse(r#"{"command":"select-participant"}"#).is_err());
    }

    #[test]
    fn snapshot_parses_what_the_script_sends() {
        let json = r#"{"version":2,"ready":true,"joined":true,"busy":false,"micEnabled":false,"micDisabled":false,"title":"x",
            "agent":"speaking","youTalking":false,"canSkip":true,"since":1759300000000,
            "participants":[{"threadId":"a","title":"x","selected":true}],"devices":{"inputs":[]},"extra":1}"#;
        let s: CallSnapshot = serde_json::from_str(json).unwrap();
        assert!(s.ready && s.joined && !s.mic_enabled && s.can_skip);
        assert_eq!(s.agent, AgentState::Speaking);
        assert_eq!(s.since, Some(1_759_300_000_000.0));
        assert_eq!(s.participants.len(), 1);
        let partial: CallSnapshot = serde_json::from_str(r#"{"ready":false}"#).unwrap();
        assert!(!partial.ready);
        assert_eq!(partial.agent, AgentState::Idle);
    }

    #[test]
    fn tray_states() {
        let not_ready = tray_view(&CallSnapshot::default(), "en");
        assert_eq!(not_ready.icon, TrayIcon::Idle);
        assert!(!not_ready.mute_enabled && !not_ready.hang_up_enabled);

        let idle = tray_view(&CallSnapshot { ready: true, mic_enabled: true, ..Default::default() }, "en");
        assert_eq!((idle.icon, idle.status.as_str()), (TrayIcon::Idle, "No call"));
        assert!(idle.mute_enabled, "mute before joining sets the preference, as in the web UI");
        assert!(!idle.hang_up_enabled);

        let on = tray_view(&live(true), "en");
        assert_eq!(
            (on.icon, on.status.as_str(), on.mute_label),
            (TrayIcon::Live, "In a call · Claude", "Mute microphone")
        );
        assert!(on.hang_up_enabled);

        let muted = tray_view(&live(false), "es");
        assert_eq!((muted.icon, muted.mute_label), (TrayIcon::Muted, "Activar micrófono"));
        assert_eq!(muted.status, "En llamada · silenciado · Claude");

        let busy = tray_view(&CallSnapshot { busy: true, ..live(true) }, "en");
        assert_eq!(busy.status, "Reconnecting…");

        let no_conversation = tray_view(&CallSnapshot { mic_disabled: true, ..live(true) }, "en");
        assert!(!no_conversation.mute_enabled);
    }

    #[test]
    fn long_titles_are_cut() {
        let s = tray_view(&CallSnapshot { title: "a".repeat(100), ..live(true) }, "en");
        assert!(s.status.ends_with('…'));
        assert!(s.status.chars().count() < 70);
    }
}
