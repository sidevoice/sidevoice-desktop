//! Desktop geometry: points on macOS, physical pixels on Windows/X11. Never mix these with page CSS pixels.
//! Every value comes from Tauri; only the space its values are read in, and whether they exist, differ per OS.
use sidevoice_desktop_core::call_controls::{Display, Point, Rect, Size};
use tauri::{AppHandle, Monitor, Runtime, WebviewWindow};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::Os;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows::Os;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::Os;

/// The units of the desktop. Each OS builds in one of them; tests use both.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy)]
enum Space {
    /// AppKit points: Tao's "physical" values are each scaled by a different display (the pointer's by the main one,
    /// a window's or monitor's by its own), so each is divided back by that display's scale.
    Points,
    /// Physical pixels, one coordinate space for every display: Tao's values as they are.
    Pixels,
}

trait Native {
    /// macOS: Points (Tao scales each value by a different display). Windows/X11: Pixels.
    const SPACE: Space;
    /// Linux overrides: Wayland hides global positions and Tao answers (0,0) instead of an error.
    fn global_coordinates() -> bool {
        true
    }
}

/// Pointer/frame coordinates, absolute placement and pointer-based resizing are usable. First call must be on the
/// main thread during setup, before workers start: Linux observes the GTK backend actually selected, including
/// XWayland, rather than environment hints.
pub fn global_coordinates() -> bool {
    Os::global_coordinates()
}

/// The pointer on the desktop.
pub fn pointer<R: Runtime>(app: &AppHandle<R>) -> Option<Point> {
    gated(global_coordinates(), || {
        let p = app.cursor_position().ok()?;
        let scale = match Os::SPACE {
            // Tao scales the global pointer by the main display, not the display beneath the pointer.
            Space::Points => app.primary_monitor().ok()??.scale_factor(),
            Space::Pixels => 1.0,
        };
        Some(Point { x: from_physical(Os::SPACE, p.x, scale), y: from_physical(Os::SPACE, p.y, scale) })
    })
}

/// The window on the desktop.
pub fn frame<R: Runtime>(w: &WebviewWindow<R>) -> Option<Rect> {
    gated(global_coordinates(), || {
        let (at, size) = (w.outer_position().ok()?, w.outer_size().ok()?);
        let scale = match Os::SPACE {
            Space::Points => w.scale_factor().ok()?,
            Space::Pixels => 1.0,
        };
        let desktop = |v: f64| from_physical(Os::SPACE, v, scale);
        Some(Rect {
            x: desktop(at.x as f64),
            y: desktop(at.y as f64),
            width: desktop(size.width as f64),
            height: desktop(size.height as f64),
        })
    })
}

/// One logical pixel of this window, in desktop units.
pub fn unit<R: Runtime>(w: &WebviewWindow<R>) -> f64 {
    match Os::SPACE {
        Space::Points => 1.0,
        Space::Pixels => w.scale_factor().unwrap_or(1.0),
    }
}

/// Place in desktop units, without changing focus; do nothing when global placement is unavailable.
pub fn place<R: Runtime>(w: &WebviewWindow<R>, at: Point) {
    gated(global_coordinates(), || {
        let _ = match Os::SPACE {
            Space::Points => w.set_position(tauri::LogicalPosition::new(at.x, at.y)),
            Space::Pixels => w.set_position(tauri::PhysicalPosition::new(at.x.round() as i32, at.y.round() as i32)),
        };
        Some(())
    });
}

/// The display with this key, else the one at `at` (on the desktop), else the room window's, else the primary.
pub fn display<R: Runtime>(
    app: &AppHandle<R>,
    room: Option<&WebviewWindow<R>>,
    key: Option<&str>,
    at: Option<Point>,
) -> Option<Display> {
    gated(global_coordinates(), || {
        let all: Vec<(Rect, Display)> =
            app.available_monitors().unwrap_or_default().iter().map(monitor_display).collect();
        if let Some(display) = selected_display(&all, key, at) {
            return Some(display);
        }
        room.and_then(|w| w.current_monitor().ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten())
            .map(|m| monitor_display(&m).1)
    })
}

/// Ask nothing, and invent nothing, when the session has no global coordinates.
fn gated<T>(available: bool, query: impl FnOnce() -> Option<T>) -> Option<T> {
    if available {
        query()
    } else {
        None
    }
}

/// A Tao "physical" value that Tao scaled by `scale`, in desktop units.
fn from_physical(space: Space, value: f64, scale: f64) -> f64 {
    match space {
        Space::Points => value / scale,
        Space::Pixels => value,
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
        Os::SPACE,
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

fn monitor_geometry(space: Space, name: &str, scale: f64, at: Point, size: Size, work: Rect) -> (Rect, Display) {
    // Desktop units per logical pixel, and per Tao pixel of this monitor.
    let (unit, to_desktop) = match space {
        Space::Points => (1.0, 1.0 / scale),
        Space::Pixels => (scale, 1.0),
    };
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

    const SPACES: [Space; 2] = [Space::Points, Space::Pixels];

    #[test]
    fn unavailable_session_never_queries_or_invents_desktop_coordinates() {
        assert!(gated::<Point>(false, || panic!("queried an unavailable session")).is_none());
        assert_eq!(gated(true, || Some(1)), Some(1));
    }

    #[test]
    fn monitor_changes_use_fresh_geometry_and_missing_keys_fall_through_to_the_pointer() {
        for space in SPACES {
            let first = monitor_geometry(
                space,
                "first",
                1.0,
                Point { x: 0.0, y: 0.0 },
                Size { width: 1920.0, height: 1080.0 },
                Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1040.0 },
            );
            let second = monitor_geometry(
                space,
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
                space,
                "first",
                1.0,
                Point { x: 0.0, y: 0.0 },
                Size { width: 1920.0, height: 1080.0 },
                Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1000.0 },
            );
            assert_eq!(selected_display(&[changed], Some(&key), None).unwrap().work.height, 1000.0);
        }
    }

    #[test]
    fn mixed_dpi_keeps_work_area_in_logical_pixels_and_round_trips_desktop_points() {
        for space in SPACES {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let (whole, display) = monitor_geometry(
                    space,
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
}
