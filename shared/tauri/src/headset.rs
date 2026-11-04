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
