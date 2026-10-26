use super::Native as Observations;
use crate::platform::desktop_geometry;

pub struct Native;

/// Linux: the page owns its cursor on X11 and there is no outside-click backend; Wayland cannot observe the pointer
/// globally, so the page's own events are used.
impl Observations for Native {
    fn available() -> bool {
        desktop_geometry::positions_available()
    }
}
