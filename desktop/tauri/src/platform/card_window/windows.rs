use super::{card_builder, CardSpec, CardWindow};
use tauri::AppHandle;
use windows_sys::Win32::{
    Foundation::{GetLastError, SetLastError, HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, STYLESTRUCT, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_NCDESTROY, WM_STYLECHANGING, WS_EX_APPWINDOW,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    },
};

pub struct NativeCardWindow;

impl CardWindow for NativeCardWindow {
    fn create(app: &AppHandle, spec: CardSpec) -> tauri::Result<()> {
        let window = card_builder(app, spec).build()?;
        // Setup runs on the window thread, before the hidden card can be shown. Tauri's skip_taskbar
        // only removes the taskbar tab; Tao retains APPWINDOW, which can still expose it in Alt+Tab.
        let result = window.hwnd().and_then(|hwnd| exclude_from_switcher(hwnd.0));
        if result.is_err() {
            let _ = window.destroy();
        }
        result
    }
}

pub(super) fn exclude_from_switcher(hwnd: HWND) -> tauri::Result<()> {
    // SAFETY: hwnd belongs to the live card on this thread. Only extended style bits are changed;
    // NOACTIVATE and every unrelated style are preserved. No pointers are retained by these calls.
    unsafe {
        // Tao rewrites GWL_EXSTYLE on show/hide. Keep the policy even across those later rewrites.
        if SetWindowSubclass(hwnd, Some(retain_card_styles), 0, 0) == 0 {
            return Err(std::io::Error::other("call-card style hook could not be installed").into());
        }
        SetLastError(0);
        let previous = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if previous == 0 && GetLastError() != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let expected = (previous | WS_EX_TOOLWINDOW as isize | WS_EX_NOACTIVATE as isize) & !(WS_EX_APPWINDOW as isize);
        SetLastError(0);
        if SetWindowLongPtrW(hwnd, GWL_EXSTYLE, expected) == 0 && GetLastError() != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // Refresh cached style data without showing, moving, restacking or activating the card.
        if SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // Construction fails closed if the native window did not retain the requested policy.
        let actual = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let policy = (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_APPWINDOW) as isize;
        if actual & policy != expected & policy {
            return Err(std::io::Error::other("call-card app-switcher exclusion was not applied").into());
        }
    }
    Ok(())
}

// No heap state or callbacks are retained. The hook exists only for this card's HWND and is removed
// at native destruction, including the failed-construction cleanup path.
unsafe extern "system" fn retain_card_styles(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: Windows provides a writable STYLESTRUCT for WM_STYLECHANGING only. Other messages
    // are forwarded untouched, and the subclass is removed before forwarding final destruction.
    unsafe {
        if message == WM_STYLECHANGING && wparam as i32 == GWL_EXSTYLE {
            let styles = &mut *(lparam as *mut STYLESTRUCT);
            styles.styleNew = (styles.styleNew | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) & !WS_EX_APPWINDOW;
        } else if message == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(retain_card_styles), id);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}
