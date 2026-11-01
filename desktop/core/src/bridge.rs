//! The app's side of the desktop bridge (docs/BRIDGE.md).
//!
//! The room page reports a [`CallSnapshot`] through the `bridge_state` command; the app drives the
//! page with a [`Command`], evaluated as a call to `window.__sidevoiceHost.run(...)`. Pure logic
//! here; the Tauri wiring lives in `lib.rs`.

use serde::{Deserialize, Serialize};

use crate::build_info::BuildInfo;

/// Removes the WebView's default context menu (desktop/core/js/no-context-menu.js); appended to the scripts of every window.
pub const NO_CONTEXT_MENU_SCRIPT: &str = include_str!("../js/no-context-menu.js");

/// Forwards the room page's console warnings and errors, uncaught errors and unhandled rejections to the app log
/// (desktop/core/js/webview-log.js); the room window only.
pub const WEBVIEW_LOG_SCRIPT: &str = include_str!("../js/webview-log.js");

/// The desktop's part of the bridge script (what only this app has, and the assembly of `host`), with its placeholders
/// still in place. The other two parts of the page's script are [`Parts`].
pub const SCRIPT_TEMPLATE: &str = include_str!("../js/desktop-bridge.js");
const ORIGIN_PLACEHOLDER: &str = "\"__SIDEVOICE_ROOM_ORIGIN__\"";
const MEDIA_PLACEHOLDER: &str = "\"__SIDEVOICE_MEDIA__\"";
const LOCAL_HOST_PLACEHOLDER: &str = "\"__SIDEVOICE_LOCAL_HOST__\"";
const VOICE_PLACEHOLDER: &str = "\"__SIDEVOICE_VOICE__\"";
const UPDATER_PLACEHOLDER: &str = "\"__SIDEVOICE_UPDATER__\"";
const BUILD_PLACEHOLDER: &str = "\"__SIDEVOICE_BUILD__\"";
const DEVICE_NAME_PLACEHOLDER: &str = "\"__SIDEVOICE_DEVICE_NAME__\"";
const MICROPHONE_SETTINGS_PLACEHOLDER: &str = "\"__SIDEVOICE_MICROPHONE_SETTINGS__\"";
const DEBUG_PLACEHOLDER: &str = "\"__SIDEVOICE_DEBUG__\"";
const UNINSTALL_PLACEHOLDER: &str = "\"__SIDEVOICE_UNINSTALL__\"";
const SYSTEM_PLACEHOLDER: &str = "\"__SIDEVOICE_OS__\"";

/// What the app tells the page it offers beyond the web, the same on every OS: this computer's own core, the local host
/// (`window.__sidevoiceHost.host.localHost`, docs/LOCAL_HOST.md), and the voice call the app runs itself
/// (`window.__sidevoiceHost.host.voice`, docs/BRIDGE.md -> "The voice call"). Both are always offered; the flags stay
/// while the script reads them.
const OFFERED: &str = "true";

/// The call controls card's bridge script, with its origin placeholder still in place.
pub const CALL_CONTROLS_SCRIPT_TEMPLATE: &str = include_str!("../js/call-controls-bridge.js");
const APP_ORIGIN_PLACEHOLDER: &str = "\"__SIDEVOICE_APP_ORIGIN__\"";
const RESIZE_PLACEHOLDER: &str = "\"__SIDEVOICE_RESIZE__\"";

/// The script injected into the call controls card's window, bound to the app's own origin. `resize`: whether the app
/// resizes the picture-in-picture by its grip (`host.callControls.resize`; not on Wayland, where it cannot see the pointer).
pub fn call_controls_script(origin: &str, resize: bool) -> String {
    let literal = serde_json::to_string(origin).expect("a string serialises");
    let bridge = CALL_CONTROLS_SCRIPT_TEMPLATE.replacen(APP_ORIGIN_PLACEHOLDER, &literal, 1).replacen(
        RESIZE_PLACEHOLDER,
        if resize { "true" } else { "false" },
        1,
    );
    format!("{bridge}\n{NO_CONTEXT_MENU_SCRIPT}")
}

/// What the app pushes to the card: any subset of `{call, level, playback, place, size, pointerInside,
/// outsideClicks, alwaysExpanded, muteShortcut}` (call-controls/host.ts `CallControlsState`).
pub fn call_controls_push(patch: &serde_json::Value) -> String {
    format!("window.__sidevoiceHost && window.__sidevoiceHost.receive({patch});")
}

/// The parts of the page's script that other crates own, as the code they ship: `transport` is the shell binding's
/// (`sidevoice-app-tauri`: how the page reaches the shared commands) and `members` the apps' (`sidevoice-app-core`: the
/// engine, the voice call, the microphone, the pairings and the call state). Each is one function expression
/// (docs/BRIDGE.md -> "Parts"). This crate names neither crate, so the app that has them hands them over.
#[derive(Debug, Clone, Copy)]
pub struct Parts<'a> {
    pub transport: &'a str,
    pub members: &'a str,
}

/// The page's script: the desktop's part, in a scope where the other two are `transportOf` and `membersOf`. The tests
/// that run the script in node (test/bridge-source.mjs) compose it the same way.
fn compose(parts: &Parts<'_>, desktop: &str) -> String {
    format!("(function (transportOf, membersOf) {{\n{desktop}\n}})({}, {});", parts.transport, parts.members)
}

/// The script injected into the main window, bound to one page origin. `updater`: whether this copy of the app updates
/// itself (`window.__sidevoiceHost.host.updater`; core `updater::offered`), which the shell finds at start; `build`:
/// what this build is (`host.build`; core `build_info`); `device_name`: the name the person gave this computer
/// (`host.deviceName`, absent with none; core `computer_name`); `debug`: the developer options kept in the settings,
/// told to the page (with the switches this platform has) only in a build that offers `host.debug` (core `debug_options`);
/// `microphone_settings`: whether the platform can open its microphone privacy page, independently of native voice.
/// `diagnostics`: native diagnostic switches the shell actually implements on this runtime.
/// `media`: whether the app owns the system's media controls (`host.presence`, offered only then), independently of native
/// voice.
/// `host.localHost` and `host.voice` are always offered ([`OFFERED`]).
/// `uninstall`: whether the app can also remove itself after the reset (`host.reset.uninstall`); delete-all and its
/// report (`host.reset.deleteAll`, `resetReport`) are on every host.
/// `system`: the system the app runs on (`host.os`; `platform::host::system`).
/// `parts`: the transport and the members, composed with the desktop's part into the one script the page gets.
#[allow(clippy::too_many_arguments)]
pub fn script_for_origin(
    origin: &str,
    updater: bool,
    build: &BuildInfo,
    device_name: Option<&str>,
    debug: &crate::debug_options::DebugOptions,
    microphone_settings: bool,
    diagnostics: &[crate::debug_options::Switch],
    media: bool,
    uninstall: bool,
    system: &str,
    parts: &Parts<'_>,
) -> String {
    let literal = serde_json::to_string(origin).expect("a string serialises");
    let flag = |offered: bool| if offered { "true" } else { "false" };
    let debug = if crate::debug_options::offered(&build.channel) {
        debug.state(diagnostics).to_string()
    } else {
        "null".to_owned()
    };
    let desktop = SCRIPT_TEMPLATE
        .replacen(ORIGIN_PLACEHOLDER, &literal, 1)
        .replacen(MEDIA_PLACEHOLDER, flag(media), 1)
        .replacen(LOCAL_HOST_PLACEHOLDER, OFFERED, 1)
        .replacen(VOICE_PLACEHOLDER, OFFERED, 1)
        .replacen(UPDATER_PLACEHOLDER, flag(updater), 1)
        .replacen(BUILD_PLACEHOLDER, &serde_json::to_string(build).expect("serialises"), 1)
        .replacen(DEVICE_NAME_PLACEHOLDER, &serde_json::to_string(&device_name).expect("serialises"), 1)
        .replacen(DEBUG_PLACEHOLDER, &debug, 1)
        .replacen(MICROPHONE_SETTINGS_PLACEHOLDER, flag(microphone_settings), 1)
        .replacen(UNINSTALL_PLACEHOLDER, flag(uninstall), 1)
        .replacen(SYSTEM_PLACEHOLDER, &serde_json::to_string(system).expect("serialises"), 1);
    compose(parts, &desktop)
}

/// The bridge's version: the room page sends it in every snapshot (desktop/core/js/desktop-bridge.js).
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
/// Mirrors the web UI's own views; `participants` and `devices` are relayed to the card as the page's bridge wrote
/// them (the web UI owns their shape: `ParticipantView`, the card's `AudioDevices`).
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
    /// The rest of the room's projection (`callCard`: `facts`, `conversation`, `mood`, `turn`, `theme`, …), which only the
    /// card reads: relayed to it as the room wrote it, never read here.
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

impl CallSnapshot {
    /// What changed since `previous`, for the app log: only the facts the app itself acts on, in a few words, with
    /// counts instead of the participants and devices and nothing of the conversation's words. `None` when none of
    /// those changed (the room reports many times a second while it animates).
    pub fn changes_since(&self, previous: &CallSnapshot) -> Option<String> {
        let mut changes = Vec::new();
        let mut flag = |name: &str, now: bool, before: bool| {
            if now != before {
                changes.push(format!("{name}={now}"));
            }
        };
        flag("ready", self.ready, previous.ready);
        flag("joined", self.joined, previous.joined);
        flag("busy", self.busy, previous.busy);
        flag("mic", self.mic_enabled, previous.mic_enabled);
        flag("mic_disabled", self.mic_disabled, previous.mic_disabled);
        flag("you_talking", self.you_talking, previous.you_talking);
        flag("can_skip", self.can_skip, previous.can_skip);
        if self.agent != previous.agent {
            changes.push(format!("agent={:?}", self.agent));
        }
        if self.title != previous.title {
            changes.push("title".to_owned());
        }
        if self.participants.len() != previous.participants.len() {
            changes.push(format!("participants={}", self.participants.len()));
        }
        if self.devices.is_some() != previous.devices.is_some() {
            changes.push(format!("devices={}", self.devices.is_some()));
        }
        (!changes.is_empty()).then(|| changes.join(" "))
    }
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
    /// The card's gear: the room's Settings at its Floating card section (`sidevoiceUI.openSettings`).
    OpenSettings,
    /// The tray's Settings: the room's Settings as it was left (`sidevoiceUI.openSettings`, no section).
    ShowSettings,
    /// Takes back the person's turn while it is recorded or transcribed (`sidevoiceActions.cancelInput`).
    CancelInput,
    /// The idle warning's Keep: a sign of the person, so the call stays on (`sidevoiceActions.keepVoiceOn`).
    KeepAlive,
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
            Command::OpenSettings => "open-settings",
            Command::ShowSettings => "show-settings",
            Command::CancelInput => "cancel-input",
            Command::KeepAlive => "keep-alive",
        }
    }

    /// The JavaScript the app evaluates in the room page. A page without the bridge ignores it. The command travels
    /// as JSON, so no value it carries can break out of the call.
    pub fn script(&self) -> String {
        let json = serde_json::to_string(self).expect("a command serialises");
        format!("window.__sidevoiceHost && window.__sidevoiceHost.run({json});")
    }
}

/// Which of the three tray icons to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIcon {
    Idle,
    Live,
    Muted,
}

/// The tray icon as decided so far, for icons decided on several threads and applied later, on the main thread
/// (sidevoice-desktop#170). The decision is taken under the slot's lock ([`decide`](Self::decide)); the tray is
/// touched only after the lock is released, and only if [`is_latest`](Self::is_latest) says no later icon was decided
/// meanwhile. The lock is never held across a tray call: from another thread those wait for the main thread, which
/// takes the same lock.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TrayIconSlot {
    decided: Option<TrayIcon>,
    latest: crate::call_controls::Latest,
}

impl TrayIconSlot {
    /// A slot whose tray already shows `icon`.
    pub fn showing(icon: TrayIcon) -> Self {
        Self { decided: Some(icon), ..Self::default() }
    }

    /// Records `icon` as the one to show. `Some(n)`: it differs from the last decided, so apply it, and `n` is the
    /// number the apply must still find to land. `None`: the tray shows it, or is about to, already.
    pub fn decide(&mut self, icon: TrayIcon) -> Option<u64> {
        if self.decided == Some(icon) {
            return None;
        }
        self.decided = Some(icon);
        Some(self.latest.advance())
    }

    /// Whether the decision numbered `n` is still the latest one.
    pub fn is_latest(&self, n: u64) -> bool {
        self.latest.is(n)
    }
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

    /// Stand-ins for the transport and the members, which other crates own.
    const PARTS: Parts<'static> = Parts { transport: "TRANSPORT", members: "MEMBERS" };

    #[test]
    fn tray_icon_slot_applies_only_changes_and_only_the_latest() {
        use TrayIcon::*;
        let mut slot = TrayIconSlot::showing(Idle);
        assert_eq!(slot.decide(Idle), None, "already shown: nothing to apply");
        let live = slot.decide(Live).expect("a change");
        assert_eq!(slot.decide(Live), None, "decided twice: applied once");
        let muted = slot.decide(Muted).expect("a change");
        assert!(!slot.is_latest(live), "a stale apply must not land after a later decision");
        assert!(slot.is_latest(muted));
        let idle = slot.decide(Idle).expect("back to the first icon is a change from the last decided");
        assert!(slot.is_latest(idle) && !slot.is_latest(muted));
    }

    #[test]
    fn tray_icon_slot_starts_undecided() {
        assert!(TrayIconSlot::default().decide(TrayIcon::Idle).is_some());
    }

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
    fn only_what_the_app_acts_on_is_a_change_and_never_the_words() {
        let before = live(true);
        assert_eq!(before.changes_since(&before.clone()), None);
        let mut same = before.clone();
        same.since = Some(5.0);
        same.rest.insert("mood".into(), serde_json::json!("calm"));
        assert_eq!(same.changes_since(&before), None, "relayed facts are not changes");
        let mut after = before.clone();
        after.mic_enabled = false;
        after.agent = AgentState::Speaking;
        after.participants = vec![serde_json::json!({"name": "Secret"})];
        after.title = "Other".into();
        let said = after.changes_since(&before).unwrap();
        assert_eq!(said, "mic=false agent=Speaking title participants=1");
    }

    #[test]
    fn every_window_script_removes_the_default_context_menu() {
        assert!(call_controls_script("tauri://localhost", true).contains("contextmenu"));
        assert!(NO_CONTEXT_MENU_SCRIPT.contains("contextmenu"));
    }

    #[test]
    fn script_binds_the_origin_as_a_js_string() {
        let dev = BuildInfo::new("dev", "0.4.0", "", "");
        let off = crate::debug_options::DebugOptions::default();
        let s = script_for_origin(
            "https://voice.example.com",
            false,
            &dev,
            None,
            &off,
            false,
            &crate::debug_options::Switch::ALL,
            true,
            false,
            "macos",
            &PARTS,
        );
        assert!(!s.contains("__SIDEVOICE_ROOM_ORIGIN__"));
        assert!(!s.contains("__SIDEVOICE_MEDIA__"));
        assert!(!s.contains("__SIDEVOICE_LOCAL_HOST__"));
        assert!(!s.contains("__SIDEVOICE_VOICE__"));
        assert!(!s.contains("__SIDEVOICE_UPDATER__"));
        assert!(!s.contains("__SIDEVOICE_BUILD__"));
        assert!(!s.contains("__SIDEVOICE_DEVICE_NAME__"));
        assert!(!s.contains("__SIDEVOICE_DEBUG__"));
        assert!(!s.contains("__SIDEVOICE_MICROPHONE_SETTINGS__"));
        assert!(!s.contains("__SIDEVOICE_UNINSTALL__"));
        assert!(!s.contains("__SIDEVOICE_OS__"));
        let platform = "true, true, true";
        let dev_build = r#"{"channel":"dev","version":"0.4.0"}"#;
        // A dev build has the developer options: what the settings keep, and the switches this platform has.
        let debug = off.state(&crate::debug_options::Switch::ALL);
        let expected = format!(
            r#"factory(window, "https://voice.example.com", {platform}, false, {dev_build}, null, {debug}, false, false, "macos")"#
        );
        assert!(s.contains(&expected), "{expected}");
        let no_diagnostics = script_for_origin(
            "https://voice.example.com",
            false,
            &dev,
            None,
            &off,
            false,
            &[],
            false,
            false,
            "linux",
            &PARTS,
        );
        assert!(no_diagnostics.contains(&off.state(&[]).to_string()));
        assert!(no_diagnostics.contains(r#"factory(window, "https://voice.example.com", false,"#));
        // A release that can update itself: the page gets `host.updater`, and `host.build` says it is a release.
        let release = BuildInfo::new("release", "0.4.0", "", "");
        let offered = script_for_origin(
            "https://voice.example.com",
            true,
            &release,
            Some("Mac de Ana"),
            &off,
            true,
            &crate::debug_options::Switch::ALL,
            true,
            true,
            "macos",
            &PARTS,
        );
        let expected = format!(
            r#"factory(window, "https://voice.example.com", {platform}, true, {{"channel":"release","version":"0.4.0"}}, "Mac de Ana", null, true, true, "macos")"#
        );
        assert!(offered.contains(&expected), "{expected}");
        // A hostile value cannot break out of the string literal.
        let evil = script_for_origin(
            "\"); alert(1); (\"",
            false,
            &dev,
            Some("\"); alert(1); (\""),
            &off,
            true,
            &crate::debug_options::Switch::ALL,
            true,
            false,
            "\"); alert(1); (\"",
            &PARTS,
        );
        assert!(evil.contains(r#"factory(window, "\"); alert(1); (\"""#));
        assert!(evil.contains(r#"{"channel":"dev","version":"0.4.0"}, "\"); alert(1); (\"", "#));
        assert!(evil.contains(r#", false, "\"); alert(1); (\"")"#));
    }

    /// The page's script is the desktop's part in a scope where the transport and the members are `transportOf` and
    /// `membersOf`, the shape test/bridge-source.mjs composes to run the same script in node.
    #[test]
    fn the_script_is_the_desktops_part_with_the_other_two_in_scope() {
        let dev = BuildInfo::new("dev", "0.4.0", "", "");
        let off = crate::debug_options::DebugOptions::default();
        let script = script_for_origin(
            "https://voice.example.com",
            false,
            &dev,
            None,
            &off,
            false,
            &[],
            false,
            false,
            "linux",
            &PARTS,
        );
        assert!(
            script.starts_with("(function (transportOf, membersOf) {\n// Sidevoice desktop bridge"),
            "{script:.120}"
        );
        assert!(script.ends_with("\n})(TRANSPORT, MEMBERS);"));
        assert_eq!(
            script.matches("TRANSPORT").count(),
            1,
            "the parts are handed over once, and not read by the desktop's"
        );
        assert_eq!(script.matches("MEMBERS").count(), 1);
    }

    #[test]
    fn the_local_host_and_the_voice_call_are_offered_on_every_os() {
        let dev = BuildInfo::new("dev", "0.4.0", "", "");
        let off = crate::debug_options::DebugOptions::default();
        let told = script_for_origin(
            "https://voice.example.com",
            false,
            &dev,
            None,
            &off,
            false,
            &[],
            true,
            false,
            "linux",
            &PARTS,
        );
        let line = r#"factory(window, "https://voice.example.com", true, true, true, false, {"channel":"dev","version":"0.4.0"}, null, "#;
        assert!(told.contains(line), "{line}");
    }

    /// The uninstall is told apart from the local host; delete-all needs no flag (every host).
    #[test]
    fn the_uninstall_offer_is_told_to_the_page_and_changes_nothing_else() {
        let dev = BuildInfo::new("dev", "0.4.0", "", "");
        let off = crate::debug_options::DebugOptions::default();
        let script = |uninstall| {
            script_for_origin(
                "https://voice.example.com",
                false,
                &dev,
                None,
                &off,
                false,
                &[],
                false,
                uninstall,
                "linux",
                &PARTS,
            )
        };
        // The factory's last arguments: the microphone settings (false here), the uninstall, then the system.
        let call = |told: &str| format!("false, {told}, \"linux\");\n}})(function installDesktopBridge(");
        let (without, with) = (script(false), script(true));
        assert!(without.contains(&call("false")) && with.contains(&call("true")));
        assert_eq!(with.replacen(&call("true"), &call("false"), 1), without);
    }

    #[test]
    fn the_card_s_script_binds_the_app_s_origin() {
        let s = call_controls_script("tauri://localhost", true);
        assert!(!s.contains("__SIDEVOICE_APP_ORIGIN__") && !s.contains("__SIDEVOICE_RESIZE__"));
        assert!(s.contains(r#"factory(window, "tauri://localhost", true)"#));
        assert!(
            call_controls_script("tauri://localhost", false).contains(r#"factory(window, "tauri://localhost", false)"#)
        );
        let push = call_controls_push(&serde_json::json!({"level": 3, "title": "\"); alert(1); (\""}));
        assert!(push.contains(r#"receive({"level":3,"title":"\"); alert(1); (\""});"#), "{push}");
    }

    #[test]
    fn commands_travel_as_json() {
        assert_eq!(
            Command::ToggleMute.script(),
            r#"window.__sidevoiceHost && window.__sidevoiceHost.run({"command":"toggle-mute"});"#
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
        assert_eq!(parse(r#"{"command":"open-settings"}"#).unwrap(), Command::OpenSettings);
        assert_eq!(parse(r#"{"command":"show-settings"}"#).unwrap(), Command::ShowSettings);
        assert_eq!(parse(r#"{"command":"cancel-input"}"#).unwrap().name(), "cancel-input");
        assert_eq!(parse(r#"{"command":"keep-alive"}"#).unwrap(), Command::KeepAlive);
    }

    #[test]
    fn snapshot_parses_what_the_script_sends() {
        let json = r#"{"version":2,"ready":true,"joined":true,"busy":false,"micEnabled":false,"micDisabled":false,"title":"x",
            "agent":"speaking","youTalking":false,"canSkip":true,"since":1759300000000,
            "participants":[{"threadId":"a","title":"x","selected":true}],"devices":{"inputs":[]},"extra":1,
            "facts":{"history":[]},"mood":"listening"}"#;
        let s: CallSnapshot = serde_json::from_str(json).unwrap();
        assert!(s.ready && s.joined && !s.mic_enabled && s.can_skip);
        assert_eq!(s.agent, AgentState::Speaking);
        assert_eq!(s.since, Some(1_759_300_000_000.0));
        assert_eq!(s.participants.len(), 1);
        // The rest of the room's projection goes to the card as it came.
        assert_eq!(s.rest.get("extra"), Some(&serde_json::json!(1)));
        let relayed = serde_json::to_value(&s).unwrap();
        assert_eq!(relayed["extra"], 1);
        assert_eq!(relayed["canSkip"], true);
        assert_eq!(relayed["facts"], serde_json::json!({"history": []}));
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
