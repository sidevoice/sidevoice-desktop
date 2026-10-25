use super::{sample, Visibility, WindowVisibility};
use sidevoice_desktop_core::call_controls::Rect;
use tauri::{Runtime, WebviewWindow};
pub struct NativeVisibility;
impl WindowVisibility for NativeVisibility {
    fn observe<R: Runtime>(window: Option<&WebviewWindow<R>>, _: Option<Rect>) -> Visibility {
        sample(window, |w| (on_screen(w), true), |_| None)
    }
}
/// Windows: DWM cloaks a window on another virtual desktop. How much other windows cover it is not measured.
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
