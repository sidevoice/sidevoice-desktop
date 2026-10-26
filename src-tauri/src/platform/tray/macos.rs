use super::{TrayAppearance, TrayIcon, TrayImage};

pub struct Appearance;

impl TrayAppearance for Appearance {
    fn image(state: TrayIcon) -> TrayImage {
        let bytes: &[u8] = match state {
            TrayIcon::Idle => include_bytes!("../../../packaging/shared/tray/idle.png"),
            TrayIcon::Live => include_bytes!("../../../packaging/shared/tray/live.png"),
            TrayIcon::Muted => include_bytes!("../../../packaging/shared/tray/muted.png"),
        };
        TrayImage { bytes, template: true }
    }
}
