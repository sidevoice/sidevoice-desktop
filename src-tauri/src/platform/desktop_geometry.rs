//! Desktop geometry: points on macOS, physical pixels on Windows/X11. Never mix these with page CSS pixels.
use sidevoice_desktop_core::call_controls::{Display, Point, Rect, Size};
use tauri::{AppHandle, Monitor, Runtime, WebviewWindow};

pub struct GeometryCapabilities {
    /// Pointer/frame coordinates, absolute placement and pointer-based resizing are usable.
    pub global_coordinates: bool,
}

pub struct NativeGeometry;
impl DesktopGeometry for NativeGeometry {}

pub trait DesktopGeometry {
    /// Read session capabilities. First call must be on the main thread during setup, before workers start.
    /// Linux observes the GTK backend actually selected, including XWayland, rather than environment hints.
    fn capabilities() -> GeometryCapabilities {
        GeometryCapabilities { global_coordinates: global_coordinates() }
    }
    // Geometry on the desktop (see `Display`): physical pixels on Windows and Linux; points on macOS, where Tao's
    // "physical" positions are each scaled by a different display (the pointer's by the main one, a window's by its own).

    /// One logical pixel of this window, in desktop units.
    fn unit<R: Runtime>(w: &WebviewWindow<R>) -> f64 {
        if cfg!(target_os = "macos") {
            1.0
        } else {
            w.scale_factor().unwrap_or(1.0)
        }
    }

    /// The pointer on the desktop.
    fn pointer<R: Runtime>(app: &AppHandle<R>) -> Option<Point> {
        if !Self::capabilities().global_coordinates {
            return None;
        }
        let p = app.cursor_position().ok()?;
        let scale = if cfg!(target_os = "macos") { app.primary_monitor().ok()??.scale_factor() } else { 1.0 };
        Some(Point { x: p.x / scale, y: p.y / scale })
    }

    /// The window on the desktop.
    fn frame<R: Runtime>(w: &WebviewWindow<R>) -> Option<Rect> {
        if !Self::capabilities().global_coordinates {
            return None;
        }
        let (at, size) = (w.outer_position().ok()?, w.outer_size().ok()?);
        let scale = if cfg!(target_os = "macos") { w.scale_factor().ok()? } else { 1.0 };
        Some(Rect {
            x: at.x as f64 / scale,
            y: at.y as f64 / scale,
            width: size.width as f64 / scale,
            height: size.height as f64 / scale,
        })
    }

    fn place<R: Runtime>(w: &WebviewWindow<R>, at: Point) {
        if !Self::capabilities().global_coordinates {
            return;
        }
        #[cfg(target_os = "macos")]
        let _ = w.set_position(tauri::LogicalPosition::new(at.x, at.y));
        #[cfg(not(target_os = "macos"))]
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
        let by_key = key.and_then(|key| all.iter().find(|(_, d)| d.key == key));
        let by_point = || at.and_then(|p| all.iter().find(|(whole, _)| whole.contains(p)));
        if let Some((_, d)) = by_key.or_else(by_point) {
            return Some(d.clone());
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

#[cfg(not(target_os = "linux"))]
fn global_coordinates() -> bool {
    true
}

#[cfg(target_os = "linux")]
fn global_coordinates() -> bool {
    use gtk::prelude::*;
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| gtk::gdk::Display::default().is_some_and(|d| d.type_().name() == "GdkX11Display"))
}

fn monitor_geometry(name: &str, scale: f64, at: Point, size: Size, work: Rect) -> (Rect, Display) {
    let (unit, to_desktop) = if cfg!(target_os = "macos") { (1.0, 1.0 / scale) } else { (scale, 1.0) };
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
