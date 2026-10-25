use super::DesktopGeometry;
use sidevoice_desktop_core::call_controls::{Point, Rect};
use tauri::{AppHandle, Runtime, WebviewWindow};

pub struct NativeGeometry;
impl DesktopGeometry for NativeGeometry {
    fn unit<R: Runtime>(_w: &WebviewWindow<R>) -> f64 {
        1.0
    }

    fn pointer<R: Runtime>(app: &AppHandle<R>) -> Option<Point> {
        let p = app.cursor_position().ok()?;
        // Tao scales the global pointer by the main display, not the display beneath the pointer.
        let scale = app.primary_monitor().ok()??.scale_factor();
        Some(Point { x: p.x / scale, y: p.y / scale })
    }

    fn frame<R: Runtime>(w: &WebviewWindow<R>) -> Option<Rect> {
        let (at, size) = (w.outer_position().ok()?, w.outer_size().ok()?);
        let scale = w.scale_factor().ok()?;
        Some(Rect {
            x: at.x as f64 / scale,
            y: at.y as f64 / scale,
            width: size.width as f64 / scale,
            height: size.height as f64 / scale,
        })
    }

    fn place<R: Runtime>(w: &WebviewWindow<R>, at: Point) {
        let _ = w.set_position(tauri::LogicalPosition::new(at.x, at.y));
    }
}

// Restore AppKit desktop points from Tao's per-monitor physical coordinates.
pub(super) fn monitor_units(scale: f64) -> (f64, f64) {
    (1.0, 1.0 / scale)
}
