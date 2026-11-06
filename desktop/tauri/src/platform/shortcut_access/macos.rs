use super::{LabelStyle, Os};
use sidevoice_desktop_core::shortcuts::Platform;

pub(super) struct Native;

impl Os for Native {
    const PLATFORM: Platform = Platform::Macos;

    const DEFAULT_MUTE: &'static str = sidevoice_desktop_core::shortcuts::DEFAULT_MUTE_MACOS;

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
