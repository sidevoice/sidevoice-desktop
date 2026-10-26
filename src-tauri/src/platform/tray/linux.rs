use super::TrayAppearance;

pub struct Appearance;

impl TrayAppearance for Appearance {
    fn creation_failed(error: tauri::Error) -> tauri::Result<()> {
        log::warn!("Tray unavailable; continuing with the room window: {error}");
        Ok(())
    }
}
