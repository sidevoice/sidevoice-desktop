//! Tray artwork and native tray availability. Menu content and actions stay in `crate::tray`.

use sidevoice_desktop_core::bridge::TrayIcon;

#[cfg(target_os = "macos")]
#[path = "tray/macos.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "tray/windows.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "tray/linux.rs"]
mod imp;

pub use imp::Appearance as SystemTrayAppearance;

/// The bytes and tinting rule are selected together, for one call state.
pub struct TrayImage {
    pub bytes: &'static [u8],
    pub template: bool,
}

pub trait TrayAppearance {
    /// Generated brand artwork for the current panel theme. Windows re-reads its taskbar theme each time;
    /// macOS tints templates itself; Linux uses the existing dark-panel fallback (no universal panel theme API),
    /// which is this default.
    fn image(state: TrayIcon) -> TrayImage {
        Self::colored(state, false)
    }

    /// Shared colored (non-template) artwork for a light or dark panel.
    fn colored(state: TrayIcon, light: bool) -> TrayImage {
        let bytes: &[u8] = match (light, state) {
            (true, TrayIcon::Idle) => include_bytes!("../../packaging/shared/tray/light-idle.png"),
            (true, TrayIcon::Live) => include_bytes!("../../packaging/shared/tray/light-live.png"),
            (true, TrayIcon::Muted) => include_bytes!("../../packaging/shared/tray/light-muted.png"),
            (false, TrayIcon::Idle) => include_bytes!("../../packaging/shared/tray/dark-idle.png"),
            (false, TrayIcon::Live) => include_bytes!("../../packaging/shared/tray/dark-live.png"),
            (false, TrayIcon::Muted) => include_bytes!("../../packaging/shared/tray/dark-muted.png"),
        };
        TrayImage { bytes, template: false }
    }

    /// Handle a failed native tray creation. macOS/Windows propagate the error; Linux logs it and keeps the
    /// room usable without a tray. A panel without AppIndicator support may simply never display the icon.
    fn creation_failed(error: tauri::Error) -> tauri::Result<()> {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_state_has_decodable_distinct_artwork() {
        let states = [TrayIcon::Idle, TrayIcon::Live, TrayIcon::Muted];
        let images = states.map(SystemTrayAppearance::image);
        for image in &images {
            let decoded = tauri::image::Image::from_bytes(image.bytes).unwrap();
            assert!(decoded.width() > 0 && decoded.height() > 0);
            assert_eq!(image.template, images[0].template);
        }
        assert_ne!(images[0].bytes, images[1].bytes);
        assert_ne!(images[1].bytes, images[2].bytes);
        assert_ne!(images[0].bytes, images[2].bytes);
    }
}
