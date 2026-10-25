//! Desktop geometry: points on macOS, physical pixels on Windows/X11. Never mix these with page CSS pixels.
use sidevoice_desktop_core::call_controls::{Display, Point, Rect, Size};
use tauri::{AppHandle, Monitor, Runtime, WebviewWindow};

pub struct GeometryCapabilities {
    /// Pointer/frame coordinates, absolute placement and pointer-based resizing are usable.
    pub global_coordinates: bool,
}

#[cfg_attr(target_os = "macos", path = "desktop_geometry/macos.rs")]
#[cfg_attr(target_os = "linux", path = "desktop_geometry/linux.rs")]
#[cfg(not(target_os = "windows"))]
mod imp;
#[cfg(not(target_os = "windows"))]
pub use imp::NativeGeometry;

/// The shared physical-pixel geometry used when an OS needs no overrides.
pub struct PhysicalGeometry;
impl DesktopGeometry for PhysicalGeometry {}
#[cfg(target_os = "windows")]
pub use PhysicalGeometry as NativeGeometry;

pub trait DesktopGeometry {
    /// Convert a monitor's scale to desktop units per logical pixel and desktop units per Tauri pixel.
    /// The default uses physical pixels; macOS overrides it to use AppKit points.
    fn monitor_units(scale: f64) -> (f64, f64) {
        (scale, 1.0)
    }

    /// Read session capabilities. First call must be on the main thread during setup, before workers start.
    /// Linux observes the GTK backend actually selected, including XWayland, rather than environment hints.
    fn capabilities() -> GeometryCapabilities {
        GeometryCapabilities { global_coordinates: true }
    }
    // Geometry on the desktop (see `Display`): physical pixels on Windows and Linux; points on macOS, where Tao's
    // "physical" positions are each scaled by a different display (the pointer's by the main one, a window's by its own).

    /// One logical pixel of this window, in desktop units.
    fn unit<R: Runtime>(w: &WebviewWindow<R>) -> f64 {
        w.scale_factor().unwrap_or(1.0)
    }

    /// The pointer on the desktop.
    fn pointer<R: Runtime>(app: &AppHandle<R>) -> Option<Point> {
        if !Self::capabilities().global_coordinates {
            return None;
        }
        let p = app.cursor_position().ok()?;
        Some(Point { x: p.x, y: p.y })
    }

    /// The window on the desktop.
    fn frame<R: Runtime>(w: &WebviewWindow<R>) -> Option<Rect> {
        if !Self::capabilities().global_coordinates {
            return None;
        }
        let (at, size) = (w.outer_position().ok()?, w.outer_size().ok()?);
        Some(Rect { x: at.x as f64, y: at.y as f64, width: size.width as f64, height: size.height as f64 })
    }

    /// Place in desktop units, without changing focus; do nothing when global placement is unavailable.
    fn place<R: Runtime>(w: &WebviewWindow<R>, at: Point) {
        if !Self::capabilities().global_coordinates {
            return;
        }
        let _ = w.set_position(tauri::PhysicalPosition::new(at.x.round() as i32, at.y.round() as i32));
    }

    /// The display with this key, else the one at `at` (on the desktop), else the room window's, else the primary.
    fn display<R: Runtime>(
        app: &AppHandle<R>,
        room: Option<&WebviewWindow<R>>,
        key: Option<&str>,
        at: Option<Point>,
    ) -> Option<Display> {
        if !Self::capabilities().global_coordinates {
            return None;
        }
        let all: Vec<(Rect, Display)> =
            app.available_monitors().unwrap_or_default().iter().map(monitor_display).collect();
        if let Some(display) = selected_display(&all, key, at) {
            return Some(display);
        }
        room.and_then(|w| w.current_monitor().ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten())
            .map(|m| monitor_display(&m).1)
    }

    /// Ask the compositor to drag when global placement is unavailable, from its pointer-down event.
    fn start_dragging<R: Runtime>(w: &WebviewWindow<R>) -> tauri::Result<()> {
        w.start_dragging()
    }
}

fn selected_display(all: &[(Rect, Display)], key: Option<&str>, at: Option<Point>) -> Option<Display> {
    let by_key = key.and_then(|key| all.iter().find(|(_, d)| d.key == key));
    let by_point = || at.and_then(|p| all.iter().find(|(whole, _)| whole.contains(p)));
    by_key.or_else(by_point).map(|(_, d)| d.clone())
}

/// A monitor's whole area on the desktop, and the display it is to the card (its work area).
fn monitor_display(m: &Monitor) -> (Rect, Display) {
    let (at, size, work) = (m.position(), m.size(), m.work_area());
    let name = m.name().cloned().unwrap_or_else(|| format!("{}x{}", size.width, size.height));
    monitor_geometry(
        &name,
        m.scale_factor(),
        Point { x: at.x as f64, y: at.y as f64 },
        Size { width: size.width as f64, height: size.height as f64 },
        Rect {
            x: work.position.x as f64,
            y: work.position.y as f64,
            width: work.size.width as f64,
            height: work.size.height as f64,
        },
    )
}

fn monitor_geometry(name: &str, scale: f64, at: Point, size: Size, work: Rect) -> (Rect, Display) {
    let (unit, to_desktop) = NativeGeometry::monitor_units(scale);
    let whole = Rect {
        x: at.x * to_desktop,
        y: at.y * to_desktop,
        width: size.width * to_desktop,
        height: size.height * to_desktop,
    };
    let display = Display::new(
        name,
        unit,
        Point { x: work.x * to_desktop, y: work.y * to_desktop },
        Size { width: work.width / scale, height: work.height / scale },
    );
    (whole, display)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Unavailable;
    impl DesktopGeometry for Unavailable {
        fn capabilities() -> GeometryCapabilities {
            GeometryCapabilities { global_coordinates: false }
        }
    }

    #[test]
    fn unavailable_session_never_queries_or_invents_desktop_coordinates() {
        let app = tauri::test::mock_app();
        let w = tauri::WebviewWindowBuilder::new(&app, "card", tauri::WebviewUrl::App("index.html".into()))
            .build()
            .unwrap();
        assert!(Unavailable::pointer(app.handle()).is_none());
        assert!(Unavailable::frame(&w).is_none());
        assert!(Unavailable::display(app.handle(), Some(&w), None, None).is_none());
        Unavailable::place(&w, Point { x: 50.0, y: 50.0 });
    }

    #[test]
    fn monitor_changes_use_fresh_geometry_and_missing_keys_fall_through_to_the_pointer() {
        let first = monitor_geometry(
            "first",
            1.0,
            Point { x: 0.0, y: 0.0 },
            Size { width: 1920.0, height: 1080.0 },
            Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1040.0 },
        );
        let second = monitor_geometry(
            "second",
            1.0,
            Point { x: 1920.0, y: 0.0 },
            Size { width: 2560.0, height: 1440.0 },
            Rect { x: 1920.0, y: 0.0, width: 2560.0, height: 1400.0 },
        );
        let key = first.1.key.clone();
        let pointer = Point { x: 2000.0, y: 100.0 };
        assert_eq!(
            selected_display(&[first.clone(), second.clone()], Some(&key), Some(pointer)),
            Some(first.1.clone())
        );
        // Unplug the saved monitor. The current pointer display wins, or None lets the caller use room/primary.
        assert_eq!(selected_display(std::slice::from_ref(&second), Some(&key), Some(pointer)), Some(second.1));
        assert!(selected_display(&[], Some(&key), Some(pointer)).is_none());
        // The same saved key gets the current work area, never a cached copy of the old monitor.
        let changed = monitor_geometry(
            "first",
            1.0,
            Point { x: 0.0, y: 0.0 },
            Size { width: 1920.0, height: 1080.0 },
            Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1000.0 },
        );
        assert_eq!(selected_display(&[changed], Some(&key), None).unwrap().work.height, 1000.0);
    }

    #[test]
    fn mixed_dpi_keeps_work_area_in_logical_pixels_and_round_trips_desktop_points() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let (whole, display) = monitor_geometry(
                "left",
                scale,
                Point { x: -2560.0, y: -200.0 },
                Size { width: 2560.0, height: 1440.0 },
                Rect { x: -2560.0, y: -160.0, width: 2560.0, height: 1400.0 },
            );
            assert_eq!(display.work.width, 2560.0 / scale);
            assert_eq!(display.work.height, 1400.0 / scale);
            let point = Point { x: 120.0, y: 80.0 };
            let desktop = display.to_desktop(point);
            let back = display.to_local(desktop);
            assert!((back.x - point.x).abs() < 1e-9);
            assert!((back.y - point.y).abs() < 1e-9);
            assert!(whole.contains(desktop));
        }
    }
}
