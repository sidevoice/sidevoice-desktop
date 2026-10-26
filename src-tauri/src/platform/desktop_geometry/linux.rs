use super::Os;
use gtk::prelude::*;

pub(super) struct Native;
impl Os for Native {
    fn positions_available() -> bool {
        // The backend GTK selected, compared by type name: `gdk::backend()` panics on an unknown backend.
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| gtk::gdk::Display::default().is_some_and(|d| d.type_().name() == "GdkX11Display"))
    }
}
