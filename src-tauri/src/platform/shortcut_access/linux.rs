use super::Backend;
use gtk::prelude::ObjectExt;

pub struct Os;

impl Backend for Os {
    fn backend_available() -> bool {
        // Inspect GTK's chosen backend, not environment hints: DISPLAY can coexist with native Wayland.
        gtk::gdk::Display::default().is_some_and(|display| display.type_().name() == "GdkX11Display")
    }
}
