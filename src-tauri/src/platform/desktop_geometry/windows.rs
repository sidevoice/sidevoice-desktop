use super::DesktopGeometry;

pub struct NativeGeometry;
impl DesktopGeometry for NativeGeometry {}

// Tauri monitor coordinates are physical pixels, as is the Windows desktop geometry contract.
pub(super) fn monitor_units(scale: f64) -> (f64, f64) {
    (scale, 1.0)
}
