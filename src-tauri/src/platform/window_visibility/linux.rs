use super::Os;
use crate::platform::desktop_geometry;
use tauri::{Runtime, WebviewWindow};

pub struct Native;

impl Os for Native {
    /// Linux has no workspace or occlusion query, so what it can say depends on the display server:
    ///
    /// - With global positions (X11), it reports nothing: the room counts as on screen unless hidden or minimised.
    /// - Without them (Wayland, which hides every window's position and workspace from the app), focus is the only
    ///   signal left, so an unfocused room counts as off screen. It is an estimate: an unfocused window may still
    ///   be visible.
    fn on_screen<R: Runtime>(w: &WebviewWindow<R>) -> Option<bool> {
        if desktop_geometry::positions_available() {
            None
        } else {
            Some(w.is_focused().unwrap_or(false))
        }
    }
}
