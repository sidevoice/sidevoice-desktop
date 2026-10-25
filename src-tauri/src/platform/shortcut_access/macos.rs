use super::{LabelStyle, ShortcutAccess};

pub struct Os;

impl ShortcutAccess for Os {
    fn reserved_chord_active(chord: &str) -> bool {
        if !sidevoice_desktop_core::settings::uses_voice_over_keys(chord) {
            return false;
        }
        use objc2::{class, msg_send, runtime::AnyObject};
        // SAFETY: the shared workspace and this public getter are available on supported macOS versions.
        unsafe {
            let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
            msg_send![workspace, isVoiceOverEnabled]
        }
    }

    fn label_style() -> LabelStyle {
        LabelStyle::Symbols
    }
}
