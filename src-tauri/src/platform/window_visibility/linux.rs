use super::{sample, Visibility, WindowVisibility};
use sidevoice_desktop_core::call_controls::Rect;
use tauri::{Runtime, WebviewWindow};
pub struct NativeVisibility;
use crate::platform::desktop_geometry::{DesktopGeometry, NativeGeometry};
impl WindowVisibility for NativeVisibility {
    fn observe<R: Runtime>(window: Option<&WebviewWindow<R>>, _: Option<Rect>) -> Visibility {
        sample(
            window,
            |w| (None, NativeGeometry::capabilities().global_coordinates || w.is_focused().unwrap_or(false)),
            |_| None,
        )
    }
}
