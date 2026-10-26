use super::Observations;
use crate::platform::desktop_geometry;
use tauri::{Runtime, WebviewWindow};

pub struct Native;

/// Linux: X11 offers no workspace or coverage observation, so the room counts as visible unless hidden or minimised.
/// Wayland offers none either (a client cannot see where it is); focus is the best available estimate, so an
/// unfocused window counts as off screen there.
impl Observations for Native {
    fn on_screen<R: Runtime>(w: &WebviewWindow<R>) -> Option<bool> {
        let wayland = !desktop_geometry::global_coordinates();
        wayland.then(|| w.is_focused().unwrap_or(false))
    }
}
