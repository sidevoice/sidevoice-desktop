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
    /// The single press mutes (the person's choice, the default), and the microphone is not yet as the system asks: the press
    /// mutes or unmutes it; `what` says which.
    Toggle(&'static str),
    /// The single press mutes, and the microphone is already as the system asks.
    Unchanged,
    /// The single press does something else the person chose (say one thing, skip, nothing): it runs, and the microphone stays
    /// as it is.
    Action,
    /// The same press already arrived as a play/pause and was acted on ([`duplicate_press`]): it is not acted on twice.
    Duplicate,
    /// No press: the system asks for the state it already holds, as it does when the app starts recording (seen on the macOS
    /// runner: two "unmute" calls of the handler as the probe's voice opened, no call on). Agreed to, and not recorded.
    Report,
    /// No call: nothing to mute.
    NoCall,
}

impl Gesture {
    /// What the system is answered (macOS's handler returns it): whether the call's microphone is now as the system asked,
    /// which the system then shows and plays as muted or unmuted. False is the system's refusal (its reject tone and banner),
    /// and the system's mute stays as it was. So it is true only where the press leaves the microphone as asked: never for a
    /// press that does something else, or the system would say "muted" while the app still listens.
    pub fn accepted(self, press_keeps_mic: bool) -> bool {
        match self {
            Gesture::Toggle(_) | Gesture::Unchanged | Gesture::Report => true,
            // The play/pause already muted or unmuted the microphone, unless the press is not the mute.
            Gesture::Duplicate => !press_keeps_mic,
            Gesture::Action | Gesture::NoCall => false,
        }
    }

    /// Whether the press goes to the app's single press (`MediaCommand::ToggleMute`, which does what the person chose).
    pub fn presses(self) -> bool {
        matches!(self, Gesture::Toggle(_) | Gesture::Action)
    }
}

/// What [`mute_gesture`] reads of the moment a mute gesture arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GestureMoment {
    /// A call is on.
    pub in_call: bool,
    /// The single press the person chose leaves the microphone alone (say one thing, skip, nothing).
    pub press_keeps_mic: bool,
    /// The call's microphone is open.
    pub mic_enabled: bool,
    /// The system's input mute, as the app last knew it (what it set, what it agreed to, what the notification said).
    pub system_muted: bool,
}

/// Whether the system's call asking for `muted` is a press at all: it asks for a state the system does not already hold.
pub fn is_press(muted: bool, system_muted: bool) -> bool {
    muted != system_muted
}

/// What the mute gesture asking for `muted` does at `now`; `duplicate` says the same press already arrived as an acted-on
/// play/pause ([`duplicate_press`], asked only of a press during a call).
pub fn mute_gesture(muted: bool, now: GestureMoment, duplicate: bool) -> Gesture {
    if !is_press(muted, now.system_muted) {
        return Gesture::Report;
    }
    if !now.in_call {
        return Gesture::NoCall;
    }
    if duplicate {
        return Gesture::Duplicate;
    }
    if now.press_keeps_mic {
        return Gesture::Action;
    }
    match mute_gesture_action(muted, now.mic_enabled) {
        Some(what) => Gesture::Toggle(what),
        None => Gesture::Unchanged,
    }
}

/// Where a headset's single press arrived from: the remote-command center's play/pause (Now Playing) or the AirPods / Beats
/// mute gesture. One press could come both ways; [`duplicate_press`] keeps it to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressSource {
    Remote,
    Gesture,
}

/// How close together a play/pause and a mute gesture are taken for the same press.
pub const DUPLICATE_WINDOW: std::time::Duration = std::time::Duration::from_millis(800);

/// Whether a single press from `source` is the same press as the last one (`last`: where it came from, whether it was acted on,
/// and how long ago), already handled: it came the other way within [`DUPLICATE_WINDOW`]. The gesture wins: a play/pause
/// after a gesture is dropped, and a gesture after a play/pause is dropped only when that play/pause did something (a play
/// with the microphone open does nothing, and the gesture is then the press). Two presses the same way are two presses.
pub fn duplicate_press(source: PressSource, last: Option<(PressSource, bool, std::time::Duration)>) -> bool {
    let Some((before, acted, ago)) = last else { return false };
    if before == source || ago >= DUPLICATE_WINDOW {
        return false;
    }
    match source {
        PressSource::Remote => true,
        PressSource::Gesture => acted,
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

    /// A moment in a call with the microphone open and the system unmuted, the single press the mute.
    fn in_call() -> GestureMoment {
        GestureMoment { in_call: true, press_keeps_mic: false, mic_enabled: true, system_muted: false }
    }

    #[test]
    fn a_mute_gesture_is_the_single_press_during_a_call_and_refused_outside_one() {
        let muted = GestureMoment { mic_enabled: false, system_muted: true, ..in_call() };
        assert_eq!(mute_gesture(true, in_call(), false), Gesture::Toggle("mute"));
        assert_eq!(mute_gesture(false, muted, false), Gesture::Toggle("unmute"));
        // The system out of step with the call: the press asks for what the call already is.
        let behind = GestureMoment { mic_enabled: false, ..in_call() };
        assert_eq!(mute_gesture(true, behind, false), Gesture::Unchanged, "already muted");
        assert!(Gesture::Unchanged.accepted(false), "agreed: the system now holds the call's state");
        let outside = GestureMoment { in_call: false, ..in_call() };
        assert_eq!(mute_gesture(true, outside, false), Gesture::NoCall);
        assert_eq!(mute_gesture(true, GestureMoment { press_keeps_mic: true, ..outside }, true), Gesture::NoCall);
        assert!(!Gesture::NoCall.accepted(false), "no call: the system says it could not");
        assert!(!Gesture::NoCall.presses());
    }

    /// The handler is also called with the state the system already holds (on the macOS runner, as the app's voice opened
    /// its input outside a call): no press, agreed to, and not refused, so no reject banner when a call starts recording.
    #[test]
    fn the_system_asking_for_the_state_it_holds_is_no_press() {
        let outside = GestureMoment { in_call: false, mic_enabled: false, ..in_call() };
        assert_eq!(mute_gesture(false, outside, false), Gesture::Report);
        assert_eq!(mute_gesture(false, in_call(), true), Gesture::Report, "not a press, so never a duplicate");
        let muted = GestureMoment { mic_enabled: false, system_muted: true, ..in_call() };
        assert_eq!(mute_gesture(true, muted, false), Gesture::Report);
        assert!(Gesture::Report.accepted(true));
        assert!(!Gesture::Report.presses());
        assert!(is_press(true, false) && is_press(false, true) && !is_press(false, false));
    }

    /// Each choice of Settings › Shortcuts › Headset, pressed twice with the microphone open: what reaches the app and what
    /// the system is told. The system is told "done" only when the microphone really is as it asked, so it never shows
    /// "muted" while the app listens, and the second press is never read as "already so" and swallowed.
    #[test]
    fn each_headset_choice_does_what_it_says_and_the_system_says_muted_only_when_it_is() {
        use sidevoice_shared_headset::HeadsetPress;
        for press in HeadsetPress::ALL {
            let keeps = press.keeps_mic();
            let start = GestureMoment { press_keeps_mic: keeps, ..in_call() };
            // The first press asks to mute the open microphone.
            let first = mute_gesture(true, start, false);
            assert!(first.presses(), "{press:?}: the press reaches the app");
            // What the system holds after it: what it asked for when agreed to, what it had when refused.
            let system_muted = first.accepted(keeps);
            let mic_open = press != HeadsetPress::Mute;
            assert_eq!(system_muted, !mic_open, "{press:?}: the system's mute is the call's after one press");
            // The second press asks for the other state than the system holds.
            let after = GestureMoment { mic_enabled: mic_open, system_muted, ..start };
            let second = mute_gesture(!system_muted, after, false);
            assert!(second.presses(), "{press:?}: the second press is not swallowed");
            match press {
                HeadsetPress::Mute => {
                    assert_eq!(first, Gesture::Toggle("mute"));
                    assert_eq!(second, Gesture::Toggle("unmute"));
                    assert!(second.accepted(keeps));
                }
                HeadsetPress::OneTurn | HeadsetPress::Skip | HeadsetPress::None => {
                    assert_eq!(first, Gesture::Action, "{press:?}: the chosen action, the microphone untouched");
                    assert_eq!(second, Gesture::Action);
                    assert!(!second.accepted(keeps), "{press:?}: the system is refused, so it stays unmuted");
                }
            }
        }
    }

    #[test]
    fn one_press_arriving_as_play_pause_and_as_the_gesture_acts_once() {
        use std::time::Duration;
        let soon = Duration::from_millis(120);
        let (remote, gesture) = (PressSource::Remote, PressSource::Gesture);
        // The gesture first: the play/pause that follows is the same press.
        assert!(duplicate_press(remote, Some((gesture, true, soon))));
        assert!(duplicate_press(remote, Some((gesture, false, soon))), "even a gesture that found nothing to change");
        // A play/pause that acted first: the gesture that follows is the same press, answered as done (the mute did it).
        assert!(duplicate_press(gesture, Some((remote, true, soon))));
        assert_eq!(mute_gesture(true, in_call(), true), Gesture::Duplicate);
        assert!(Gesture::Duplicate.accepted(false), "the play/pause already muted");
        assert!(!Gesture::Duplicate.accepted(true), "a press that is not the mute never says muted");
        assert!(!Gesture::Duplicate.presses(), "not sent to the app twice");
        // A play/pause that did nothing (play while open): the gesture is the press.
        assert!(!duplicate_press(gesture, Some((remote, false, soon))));
        // Two presses the same way, a press with nothing before, or the other way but later, are presses of their own.
        assert!(!duplicate_press(remote, Some((remote, true, soon))));
        assert!(!duplicate_press(gesture, Some((gesture, true, soon))));
        assert!(!duplicate_press(gesture, None));
        assert!(!duplicate_press(remote, Some((gesture, true, DUPLICATE_WINDOW))));
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
