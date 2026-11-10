//! What a headset button does to the call's microphone (`platform/media_controls/apple.rs` wires it to macOS and iOS,
//! `platform/media_controls/android.rs` to Android's media session). What a press does, and what the page reads of it, is
//! `sidevoice-shared-headset`'s.

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
/// press's (`play`, `pause`, `toggle`) and, where the platform registers it (`next_track`: macOS), the
/// double press's `next`.
pub fn registered_commands(next_track: bool) -> Vec<&'static str> {
    let mut names = vec!["play", "pause", "toggle"];
    if next_track {
        names.push("next");
    }
    names
}

/// How long after the app set the system's input mute itself (`setInputMuted`) a report of that same state is its echo.
pub const ECHO_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether the input mute the system reports (`muted`) is the echo of the app's own `setInputMuted` (`set`: the state it set,
/// and how long ago): the same state within [`ECHO_WINDOW`]. An echo is neither recorded nor acted on, and is taken once.
pub fn is_echo(set: Option<(bool, std::time::Duration)>, muted: bool) -> bool {
    matches!(set, Some((value, ago)) if value == muted && ago < ECHO_WINDOW)
}

/// What the AirPods / Beats mute gesture's listening does at a report of the call ([`gesture_step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GestureStep {
    /// Start hearing it: a call is on and it is not heard yet (opting in at launch did not take).
    pub listen: bool,
    /// Give the system's input mute back unmuted: the call that held it in step has ended.
    pub give_back: bool,
}

/// What the gesture's listening does now. It is heard for the process's life, from launch, on macOS and iOS alike: the system
/// gives the gesture only to a process that records *and* has opted in, and observing the input-mute notification is that
/// opt-in on both (on macOS a handler alone is refused: "Cannot Control Mic with AirPods"). So a call's end never stops it,
/// and a call only tries again when launch did not take. `on`, it is heard; `active`, a call is on; `held`, the last call held
/// the system's mute in step.
pub fn gesture_step(on: bool, active: bool, held: bool) -> GestureStep {
    GestureStep { listen: active && !on, give_back: !active && held }
}

/// What a mute gesture does to the call ([`mute_gesture`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// The headset's single press, which does what the person chose for it; `what` is the state the system asked for.
    Press(&'static str),
    /// In a call, and the microphone is already as the system asks.
    Unchanged,
    /// No call: nothing to mute.
    NoCall,
}

impl Gesture {
    /// What the system is answered (macOS's handler returns it): whether the call's microphone is now as asked. False only
    /// with no call, which the system shows as a mute it could not do.
    pub fn accepted(self) -> bool {
        !matches!(self, Gesture::NoCall)
    }
}

/// What the mute gesture asking for `muted` does, given whether a call is on and its microphone's state now.
pub fn mute_gesture(in_call: bool, muted: bool, mic_enabled: bool) -> Gesture {
    if !in_call {
        return Gesture::NoCall;
    }
    match mute_gesture_action(muted, mic_enabled) {
        Some(what) => Gesture::Press(what),
        None => Gesture::Unchanged,
    }
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
        assert_eq!(registered_commands(false), ["play", "pause", "toggle"], "a platform with no double press");
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
    fn only_the_same_state_shortly_after_the_app_set_it_is_its_echo() {
        use std::time::Duration;
        assert!(is_echo(Some((true, Duration::from_millis(40))), true), "the app muted: iOS reports it back");
        assert!(!is_echo(Some((true, Duration::from_millis(40))), false), "the other state is a press");
        assert!(!is_echo(Some((false, ECHO_WINDOW)), false), "too late: a press");
        assert!(!is_echo(None, true), "the app set nothing: a press");
    }

    #[test]
    fn the_mute_gesture_is_heard_from_launch_for_life_and_its_mute_is_given_back_when_a_call_ends() {
        let step = |listen, give_back| GestureStep { listen, give_back };
        // Opted in at launch (macOS and iOS): no call starts or stops it, so it is there before the call records.
        assert_eq!(gesture_step(true, false, false), GestureStep::default(), "before any call");
        assert_eq!(gesture_step(true, true, false), GestureStep::default(), "a call starts, already heard");
        assert_eq!(gesture_step(true, true, true), GestureStep::default(), "the call goes on");
        assert_eq!(gesture_step(true, false, true), step(false, true), "the call ends: unmuted, and still heard");
        assert_eq!(gesture_step(true, false, false), GestureStep::default(), "between calls");
        // Launch did not take: a call tries again, and its end gives the mute back.
        assert_eq!(gesture_step(false, true, false), step(true, false), "a call tries again");
        assert_eq!(gesture_step(false, false, false), GestureStep::default(), "no call and not heard: nothing");
    }

    #[test]
    fn a_mute_gesture_is_the_single_press_during_a_call_and_refused_outside_one() {
        assert_eq!(mute_gesture(true, true, true), Gesture::Press("mute"));
        assert_eq!(mute_gesture(true, false, false), Gesture::Press("unmute"));
        assert_eq!(mute_gesture(true, true, false), Gesture::Unchanged, "already muted");
        assert_eq!(mute_gesture(true, false, true), Gesture::Unchanged, "already open");
        assert_eq!(mute_gesture(false, true, true), Gesture::NoCall);
        assert!(mute_gesture(true, true, true).accepted());
        assert!(mute_gesture(true, true, false).accepted(), "already as asked: done");
        assert!(!mute_gesture(false, false, false).accepted(), "no call: the system says it could not");
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
