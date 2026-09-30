//! What a headset button does to the call's microphone (the app's `src/headset.rs` wires it to macOS).

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
    fn the_mute_gesture_only_acts_when_it_changes_something() {
        assert_eq!(mute_gesture_action(true, true), Some("mute"));
        assert_eq!(mute_gesture_action(false, false), Some("unmute"));
        assert_eq!(mute_gesture_action(true, false), None);
        assert_eq!(mute_gesture_action(false, true), None);
    }
}
