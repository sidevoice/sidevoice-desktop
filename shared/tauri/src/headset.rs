//! What a headset button does to the call's microphone (`platform/media_controls/apple.rs` wires it to macOS and iOS,
//! `platform/media_controls/android.rs` to Android's media session).

/// What a media command does to the microphone, given whether it is on now. `None`: leave it.
pub fn media_command_action(command: &str, mic_enabled: bool) -> Option<&'static str> {
    match command {
        "play" if !mic_enabled => Some("unmute"),
        "play" => None,
        "pause" | "toggle" => Some(if mic_enabled { "mute" } else { "unmute" }),
        _ => None,
    }
}

/// The commands of the system's remote-command center the app answers while it shows a call as Now Playing: the single
/// press's (`play`, `pause`, `toggle`) and, where the platform has it (`next_track`: macOS), the double press's `next`.
pub fn registered_commands(next_track: bool) -> Vec<&'static str> {
    let mut names = vec!["play", "pause", "toggle"];
    if next_track {
        names.push("next");
    }
    names
}

/// Every command of the command center the app lets go of when Now Playing ends: those it registers, and the rest, which
/// any target or enabled state of would keep the app the Now Playing app.
pub const ALL_COMMANDS: [&str; 20] = [
    "play",
    "pause",
    "toggle",
    "stop",
    "next",
    "previous",
    "skipForward",
    "skipBackward",
    "seekForward",
    "seekBackward",
    "changePlaybackPosition",
    "changePlaybackRate",
    "changeRepeatMode",
    "changeShuffleMode",
    "rating",
    "like",
    "dislike",
    "bookmark",
    "enableLanguageOption",
    "disableLanguageOption",
];

/// What a media command asks of the call, given whether one is on and the microphone's state now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch {
    /// The headset's double press (`next`) during a call.
    NextTrack,
    /// Mute or unmute: `what` is which.
    ToggleMute(&'static str),
    /// In a call, and the microphone is already as the command asks.
    Unchanged,
    /// No call: only logged.
    NoCall,
}

/// Where a media command goes: `next` is the double press, the others the single press's rule ([`media_command_action`]).
pub fn media_dispatch(command: &str, in_call: bool, mic_enabled: bool) -> Dispatch {
    if !in_call {
        return Dispatch::NoCall;
    }
    if command == "next" {
        return Dispatch::NextTrack;
    }
    match media_command_action(command, mic_enabled) {
        Some(what) => Dispatch::ToggleMute(what),
        None => Dispatch::Unchanged,
    }
}

/// What the AirPods gesture does, given the state it asks for and the microphone's now. `None`: already so.
pub fn mute_gesture_action(muted: bool, mic_enabled: bool) -> Option<&'static str> {
    match (muted, mic_enabled) {
        (true, true) => Some("mute"),
        (false, false) => Some("unmute"),
        _ => None,
    }
}

/// Whether a line of the log says something new: false for the line last seen. The log goes to a file, and the call reports
/// again on every change of anything in it, so what has not changed is not written again.
#[derive(Default)]
pub struct Changed(pub std::sync::Mutex<Option<String>>);

impl Changed {
    pub fn is_new(&self, line: &str) -> bool {
        let mut last = self.0.lock().unwrap();
        if last.as_deref() == Some(line) {
            return false;
        }
        *last = Some(line.to_owned());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_commands_follow_the_web_interface_s_rule() {
        assert_eq!(media_command_action("play", false), Some("unmute"));
        assert_eq!(media_command_action("play", true), None, "play always means open, never close");
        assert_eq!(media_command_action("pause", true), Some("mute"));
        assert_eq!(media_command_action("pause", false), Some("unmute"), "a click may arrive as pause while muted");
        assert_eq!(media_command_action("toggle", true), Some("mute"));
        assert_eq!(media_command_action("skip", true), None);
    }

    /// The register and release sets of `MPRemoteCommandCenter` as pure logic: the real registration needs a Mac, but which
    /// commands are registered, that the double press's is one of them only where the platform has it, and that releasing
    /// covers every one of them are decided here.
    #[test]
    fn the_double_press_command_is_registered_only_where_the_platform_has_it_and_is_released_with_the_rest() {
        assert_eq!(
            registered_commands(false),
            ["play", "pause", "toggle"],
            "iOS: no next-track button on the lock screen"
        );
        assert_eq!(
            registered_commands(true),
            ["play", "pause", "toggle", "next"],
            "macOS: the double press arrives as next track"
        );
        for name in registered_commands(true) {
            assert!(ALL_COMMANDS.contains(&name), "{name} is registered, so it must be released");
        }
        let mut seen = std::collections::HashSet::new();
        assert!(ALL_COMMANDS.iter().all(|name| seen.insert(*name)), "each command once");
    }

    #[test]
    fn next_track_is_the_double_press_during_a_call_and_only_logged_outside_one() {
        assert_eq!(media_dispatch("next", true, true), Dispatch::NextTrack);
        assert_eq!(media_dispatch("next", true, false), Dispatch::NextTrack, "whatever the microphone is");
        assert_eq!(media_dispatch("next", false, true), Dispatch::NoCall);
        assert_eq!(media_dispatch("toggle", true, true), Dispatch::ToggleMute("mute"));
        assert_eq!(media_dispatch("play", true, true), Dispatch::Unchanged);
        assert_eq!(media_dispatch("pause", false, true), Dispatch::NoCall);
    }

    #[test]
    fn a_line_is_new_until_it_repeats() {
        let seen = Changed::default();
        assert!(seen.is_new("a"));
        assert!(!seen.is_new("a"));
        assert!(seen.is_new("b"));
        assert!(seen.is_new("a"));
    }

    #[test]
    fn the_mute_gesture_only_acts_when_it_changes_something() {
        assert_eq!(mute_gesture_action(true, true), Some("mute"));
        assert_eq!(mute_gesture_action(false, false), Some("unmute"));
        assert_eq!(mute_gesture_action(true, false), None);
        assert_eq!(mute_gesture_action(false, true), None);
    }
}
