use super::Os;
use sidevoice_desktop_core::call_controls::{self as logic, Rect};
use tauri::{Runtime, WebviewWindow};

pub struct Native;

/// Windows: DWM cloaks a window on another virtual desktop; the windows above the room in z-order say how much of the
/// stage other apps cover (as on macOS), so the card shows when another app is put over the room, not only when it is
/// minimised. WebView2 raises no `visibilitychange` for a covered window, so the page cannot tell.
impl Os for Native {
    fn on_screen<R: Runtime>(w: &WebviewWindow<R>) -> Option<bool> {
        use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
        let hwnd = w.hwnd().ok()?;
        let mut cloaked: u32 = 0;
        // SAFETY: the room's own window handle, and a u32 as large as the attribute.
        let read = unsafe {
            DwmGetWindowAttribute(
                hwnd.0 as _,
                DWMWA_CLOAKED as _,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        (read == 0).then_some(cloaked == 0)
    }

    fn uncovered<R: Runtime>(w: &WebviewWindow<R>, stage: Option<Rect>) -> Option<f64> {
        uncovered(w, stage)
    }
}

/// How a window's layering lets the room show through it. Only a layered window whose global alpha is the whole of its
/// transparency, fully opaque, counts as covering: colour-keyed, translucent, per-pixel or unreadable ones are overlays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layer {
    /// Not layered.
    Plain,
    /// Layered, read back as fully opaque with no colour key.
    Opaque,
    /// Layered, anything else: translucent, colour-keyed, per-pixel alpha or not readable.
    Other,
}

/// The most windows the z-order walk looks at: the walk is on the main thread and must end however the order changes.
const MAX_WALK: usize = 512;

struct Above {
    visible: bool,
    iconic: bool,
    cloaked: bool,
    own: bool,
    no_activate: bool,
    tool: bool,
    transparent: bool,
    layer: Layer,
}

/// Whether a window above the room hides part of it: visible, not minimised, not cloaked, another process's (the card
/// is ours), an ordinary window (not a tool window, never-activating or click-through overlay) and not a see-through
/// layered one. The taskbar, tooltips and palettes do not count, as the Dock and menus do not on macOS.
fn covers(w: &Above) -> bool {
    w.visible
        && !w.iconic
        && !w.cloaked
        && !w.own
        && !w.no_activate
        && !w.tool
        && !w.transparent
        && w.layer != Layer::Other
}

/// The part of the stage (physical pixels on the desktop) no other app's window above the room covers, walking the
/// z-order upward from the room's window. Bounds are DWM's extended frame, without the invisible resize border.
fn uncovered<R: Runtime>(w: &WebviewWindow<R>, stage: Option<Rect>) -> Option<f64> {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetLayeredWindowAttributes, GetWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        GWL_EXSTYLE, GW_HWNDPREV, LWA_ALPHA, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };

    let scale = w.scale_factor().ok()?;
    let (at, size) = (w.inner_position().ok()?, w.inner_size().ok()?);
    let content = Rect { x: at.x as f64, y: at.y as f64, width: size.width as f64, height: size.height as f64 };
    let target = match stage {
        Some(s) => Rect {
            x: content.x + s.x * scale,
            y: content.y + s.y * scale,
            width: s.width * scale,
            height: s.height * scale,
        },
        None => content,
    };
    let room = w.hwnd().ok()?.0 as HWND;
    let own_pid = std::process::id();
    let mut above = Vec::new();
    // SAFETY: plain queries on window handles taken from the system's own z-order; a handle that dies meanwhile only
    // makes a query fail, and the walk is bounded (MAX_WALK).
    unsafe {
        let mut next = GetWindow(room, GW_HWNDPREV);
        // Bounded, and ends at the room's own window should the order loop back to it.
        for _ in 0..MAX_WALK {
            if next.is_null() || next == room {
                break;
            }
            let hwnd = next;
            next = GetWindow(hwnd, GW_HWNDPREV);
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let mut cloaked: u32 = 0;
            DwmGetWindowAttribute(
                hwnd as _,
                DWMWA_CLOAKED as _,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            let layer = if ex & WS_EX_LAYERED == 0 {
                Layer::Plain
            } else {
                let (mut alpha, mut flags) = (0u8, 0u32);
                let read = GetLayeredWindowAttributes(hwnd, std::ptr::null_mut(), &mut alpha, &mut flags);
                if read != 0 && flags == LWA_ALPHA && alpha == 255 {
                    Layer::Opaque
                } else {
                    Layer::Other
                }
            };
            let seen = Above {
                visible: IsWindowVisible(hwnd) != 0,
                iconic: IsIconic(hwnd) != 0,
                cloaked: cloaked != 0,
                own: pid == own_pid,
                no_activate: ex & WS_EX_NOACTIVATE != 0,
                tool: ex & WS_EX_TOOLWINDOW != 0,
                transparent: ex & WS_EX_TRANSPARENT != 0,
                layer,
            };
            if !covers(&seen) {
                continue;
            }
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let read = DwmGetWindowAttribute(
                hwnd as _,
                DWMWA_EXTENDED_FRAME_BOUNDS as _,
                (&mut r as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            );
            if read == 0 {
                above.push(Rect {
                    x: r.left as f64,
                    y: r.top as f64,
                    width: (r.right - r.left) as f64,
                    height: (r.bottom - r.top) as f64,
                });
            }
        }
    }
    Some(logic::uncovered(target, &above))
}

#[cfg(test)]
mod tests {
    use super::{covers, Above, Layer};

    fn ordinary() -> Above {
        Above {
            visible: true,
            iconic: false,
            cloaked: false,
            own: false,
            no_activate: false,
            tool: false,
            transparent: false,
            layer: Layer::Plain,
        }
    }

    #[test]
    fn only_an_ordinary_visible_window_of_another_app_covers() {
        assert!(covers(&ordinary()));
        assert!(covers(&Above { layer: Layer::Opaque, ..ordinary() }), "opaque layered");
        assert!(!covers(&Above { visible: false, ..ordinary() }), "hidden");
        assert!(!covers(&Above { iconic: true, ..ordinary() }), "minimised");
        assert!(!covers(&Above { cloaked: true, ..ordinary() }), "on another desktop");
        assert!(!covers(&Above { own: true, ..ordinary() }), "the card");
        assert!(!covers(&Above { no_activate: true, ..ordinary() }), "taskbar, tooltip");
    }

    #[test]
    fn overlays_and_tool_windows_do_not_cover() {
        assert!(!covers(&Above { tool: true, ..ordinary() }), "floating palette");
        assert!(!covers(&Above { transparent: true, ..ordinary() }), "click-through overlay");
        assert!(
            !covers(&Above { layer: Layer::Other, ..ordinary() }),
            "translucent, colour-keyed or unreadable layered"
        );
    }
}
