use super::{TrayAppearance, TrayIcon, TrayImage};

pub struct Appearance;

impl TrayAppearance for Appearance {
    fn image(state: TrayIcon) -> TrayImage {
        let bytes: &[u8] = match state {
            TrayIcon::Idle => include_bytes!("../../../icons/tray/idle.png"),
            TrayIcon::Live => include_bytes!("../../../icons/tray/live.png"),
            TrayIcon::Muted => include_bytes!("../../../icons/tray/muted.png"),
        };
        TrayImage { bytes, template: true }
    }
}
