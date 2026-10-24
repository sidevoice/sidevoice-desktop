use super::{TrayAppearance, TrayIcon, TrayImage};

pub struct Appearance;

impl TrayAppearance for Appearance {
    fn image(state: TrayIcon) -> TrayImage {
        super::colored(state, false)
    }

    fn creation_failed(error: tauri::Error) -> tauri::Result<()> {
        log::warn!("Tray unavailable; continuing with the room window: {error}");
        Ok(())
    }
}
