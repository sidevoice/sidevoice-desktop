//! Tray artwork and native tray availability. Menu content and actions stay in `crate::tray`.

use sidevoice_desktop_core::bridge::TrayIcon;

#[cfg_attr(target_os = "macos", path = "tray/macos.rs")]
#[cfg_attr(target_os = "windows", path = "tray/windows.rs")]
#[cfg_attr(target_os = "linux", path = "tray/linux.rs")]
mod imp;
use imp::Native;

/// The bytes and tinting rule are selected together, for one call state.
pub struct TrayImage {
    pub bytes: &'static [u8],
    pub template: bool,
}

/// What differs per OS about the tray.
trait Os {
    /// Whether the system tints template artwork itself (macOS).
    const TEMPLATE_ART: bool = false;

    /// Whether the room stays usable without a tray: a panel may lack AppIndicator support (Linux).
    const TRAY_OPTIONAL: bool = false;

    /// Whether the panel is light, read each time; `None` where the OS has no panel theme to read.
    fn light_panel() -> Option<bool> {
        None
    }
}

/// Generated brand artwork for the current panel theme. Windows re-reads its taskbar theme each time;
/// macOS tints templates itself; Linux uses the dark-panel fallback (no universal panel theme API).
pub fn image(state: TrayIcon) -> TrayImage {
    if Native::TEMPLATE_ART {
        let bytes: &[u8] = match state {
            TrayIcon::Idle => include_bytes!("../../packaging/shared/tray/idle.png"),
            TrayIcon::Live => include_bytes!("../../packaging/shared/tray/live.png"),
            TrayIcon::Muted => include_bytes!("../../packaging/shared/tray/muted.png"),
        };
        return TrayImage { bytes, template: true };
    }
    let bytes: &[u8] = match (Native::light_panel().unwrap_or(false), state) {
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
pub fn creation_failed(error: tauri::Error) -> tauri::Result<()> {
    if !Native::TRAY_OPTIONAL {
        return Err(error);
    }
    log::warn!("Tray unavailable; continuing with the room window: {error}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_state_has_decodable_distinct_artwork() {
        let states = [TrayIcon::Idle, TrayIcon::Live, TrayIcon::Muted];
        let images = states.map(image);
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
