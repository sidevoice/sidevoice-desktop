use super::{DesktopGeometry, GeometryCapabilities};
use gtk::prelude::*;

pub struct NativeGeometry;
impl DesktopGeometry for NativeGeometry {
    fn capabilities() -> GeometryCapabilities {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let available = *AVAILABLE
            .get_or_init(|| gtk::gdk::Display::default().is_some_and(|d| d.type_().name() == "GdkX11Display"));
        GeometryCapabilities { global_coordinates: available }
    }
}

// X11 uses physical pixels. Wayland returns no global geometry before this conversion is reached.
pub(super) fn monitor_units(scale: f64) -> (f64, f64) {
    (scale, 1.0)
}
