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

/// Whether a window above the room hides part of it: visible, not minimised, not cloaked, another process's (the card
/// is ours), able to take activation and not click-through. The taskbar, tooltips and overlays that never activate do
/// not count, as the Dock and menus do not on macOS.
fn covers(
    visible: bool,
    iconic: bool,
    cloaked: bool,
    own: bool,
    no_activate: bool,
    transparent: bool,
    alpha: u8,
) -> bool {
    visible && !iconic && !cloaked && !own && !no_activate && !transparent && alpha > 0
}

/// The part of the stage (physical pixels on the desktop) no other app's window above the room covers, walking the
/// z-order upward from the room's window. Bounds are DWM's extended frame, without the invisible resize border.
fn uncovered<R: Runtime>(w: &WebviewWindow<R>, stage: Option<Rect>) -> Option<f64> {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetLayeredWindowAttributes, GetWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        GWL_EXSTYLE, GW_HWNDPREV, LWA_ALPHA, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
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
    // makes a query fail, and the walk ends at the first null.
    unsafe {
        let mut next = GetWindow(room, GW_HWNDPREV);
        while !next.is_null() {
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
            let mut alpha = 255u8;
            if ex & WS_EX_LAYERED != 0 {
                let (mut a, mut flags) = (255u8, 0u32);
                if GetLayeredWindowAttributes(hwnd, std::ptr::null_mut(), &mut a, &mut flags) != 0
                    && flags & LWA_ALPHA != 0
                {
                    alpha = a;
                }
            }
            if !covers(
                IsWindowVisible(hwnd) != 0,
                IsIconic(hwnd) != 0,
                cloaked != 0,
                pid == own_pid,
                ex & WS_EX_NOACTIVATE != 0,
                ex & WS_EX_TRANSPARENT != 0,
                alpha,
            ) {
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
    use super::covers;

    #[test]
    fn only_an_ordinary_visible_window_of_another_app_covers() {
        assert!(covers(true, false, false, false, false, false, 255));
        assert!(!covers(false, false, false, false, false, false, 255), "hidden");
        assert!(!covers(true, true, false, false, false, false, 255), "minimised");
        assert!(!covers(true, false, true, false, false, false, 255), "on another desktop");
        assert!(!covers(true, false, false, true, false, false, 255), "the card");
        assert!(!covers(true, false, false, false, true, false, 255), "never activates: taskbar, tooltip");
        assert!(!covers(true, false, false, false, false, true, 255), "click-through overlay");
        assert!(!covers(true, false, false, false, false, false, 0), "fully transparent");
    }
}
