use super::Os;
use gtk::prelude::ObjectExt;

pub(super) struct Native;

impl Os for Native {
    fn backend_available() -> bool {
        // Inspect GTK's chosen backend, not environment hints: DISPLAY can coexist with native Wayland.
        gtk::gdk::Display::default().is_some_and(|display| display.type_().name() == "GdkX11Display")
    }
}
