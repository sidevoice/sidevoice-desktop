use super::{sample, Visibility, WindowVisibility};
use sidevoice_desktop_core::call_controls::{self as logic, Rect};
use tauri::{Runtime, WebviewWindow};
pub struct NativeVisibility;
impl WindowVisibility for NativeVisibility {
    fn observe<R: Runtime>(window: Option<&WebviewWindow<R>>, stage: Option<Rect>) -> Visibility {
        sample(window, |w| (on_screen(w), false), |w| uncovered(w, stage))
    }
}
/// macOS: AppKit's occlusion state, clear while no part of the window is on screen (fully covered, or on another Space).
fn on_screen<R: Runtime>(w: &WebviewWindow<R>) -> Option<bool> {
    use tauri_nspanel::objc2::{msg_send, runtime::AnyObject};
    // NSWindowOcclusionStateVisible: set while any part of the window is on screen.
    const VISIBLE: usize = 1 << 1;
    // SAFETY: the window's own NSWindow, read on the main thread ([`system_shows`]).
    w.ns_window().ok().map(|ns| unsafe {
        let state: usize = msg_send![ns as *mut AnyObject, occlusionState];
        state & VISIBLE != 0
    })
}

/// macOS: the part of the stage (on the desktop) no other app's window above the room covers, from the window server's
/// list of the windows in front of it. Ordinary and floating windows count; the Dock, menus, the menu bar and the app's
/// own windows (the card) do not. Window bounds need no permission (only their titles would).
fn uncovered<R: Runtime>(w: &WebviewWindow<R>, stage: Option<Rect>) -> Option<f64> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::geometry::CGRect;
    use core_graphics::window::{
        copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenAboveWindow,
    };
    use tauri_nspanel::objc2::{msg_send, runtime::AnyObject};

    // kCGWindowListOptionOnScreenAboveWindow | kCGWindowListExcludeDesktopElements.
    const ABOVE: u32 = kCGWindowListOptionOnScreenAboveWindow | kCGWindowListExcludeDesktopElements;
    // kCGDockWindowLevel: the Dock and what floats over it (menus, the menu bar, the card) cover no window.
    const DOCK_LEVEL: i64 = 20;

    // The room's content on the desktop in points, top-left origin: as the window server gives windows' bounds.
    let scale = w.scale_factor().ok()?;
    let (at, size) = (w.inner_position().ok()?, w.inner_size().ok()?);
    let content = Rect {
        x: at.x as f64 / scale,
        y: at.y as f64 / scale,
        width: size.width as f64 / scale,
        height: size.height as f64 / scale,
    };
    let target = match stage {
        Some(s) => Rect { x: content.x + s.x, y: content.y + s.y, width: s.width, height: s.height },
        None => content,
    };
    // SAFETY: the room's own NSWindow, on the main thread.
    let number = w.ns_window().ok().map(|ns| -> isize { unsafe { msg_send![ns as *mut AnyObject, windowNumber] } })?;
    let raw = copy_window_info(ABOVE, u32::try_from(number).ok()?)?;
    // SAFETY: the window server's list of window-info dictionaries (string keys, CF values); the extra retain is released
    // with the wrapper.
    let windows: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_get_rule(raw.as_concrete_TypeRef()) };
    let own = i64::from(std::process::id());
    let number_of = |d: &CFDictionary<CFString, CFType>, key: &'static str| {
        d.find(CFString::from_static_string(key)).and_then(|v| v.downcast::<CFNumber>())
    };
    let above: Vec<Rect> = windows
        .iter()
        .filter_map(|d| {
            let layer = number_of(&d, "kCGWindowLayer")?.to_i64()?;
            let pid = number_of(&d, "kCGWindowOwnerPID")?.to_i64()?;
            let alpha = number_of(&d, "kCGWindowAlpha").and_then(|a| a.to_f64()).unwrap_or(1.0);
            if !(0..DOCK_LEVEL).contains(&layer) || pid == own || alpha <= 0.0 {
                return None;
            }
            let bounds = d.find(CFString::from_static_string("kCGWindowBounds"))?.downcast::<CFDictionary>()?;
            let r = CGRect::from_dict_representation(&bounds)?;
            Some(Rect { x: r.origin.x, y: r.origin.y, width: r.size.width, height: r.size.height })
        })
        .collect();
    Some(logic::uncovered(target, &above))
}
