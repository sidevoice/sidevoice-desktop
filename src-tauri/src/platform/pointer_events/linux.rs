use super::{polling, PointerEvents, Sink, State, Subscription};
use crate::platform::desktop_geometry::{DesktopGeometry, NativeGeometry};
use tauri::AppHandle;
pub struct NativePointerEvents;
impl PointerEvents for NativePointerEvents {
    fn watch(app: &AppHandle, label: &'static str, margin: f64, state: State, sink: Sink) -> Option<Subscription> {
        if !NativeGeometry::capabilities().global_coordinates {
            return None;
        }
        // The page owns its cursor on X11; Linux has no outside-click backend.
        Some(polling(app, label, margin, state, sink, None, None))
    }
}
