use gtk::prelude::*;

use super::{Confirmation, ConfirmationKind, DialogStart};

pub struct NativeConfirmation;

impl Confirmation for NativeConfirmation {
    fn show(kind: ConfirmationKind, start: DialogStart) -> bool {
        // Tauri normally initializes GTK on its UI thread. Do not construct a widget if that prerequisite
        // or the display connection is missing: GTK would abort rather than return a recoverable error.
        if !gtk::is_initialized_main_thread() || gtk::gdk::Display::default().is_none() {
            return false;
        }
        let locale = sys_locale::get_locale().unwrap_or_default();
        let (title, detail, cancel, action) = kind.words(&locale);
        let dialog = gtk::MessageDialog::builder()
            .title(title)
            .text(title)
            .secondary_text(detail)
            .message_type(gtk::MessageType::Warning)
            .modal(true)
            .build();
        dialog.add_button(cancel, gtk::ResponseType::Cancel);
        dialog.add_button(action, gtk::ResponseType::Accept);
        dialog.set_default_response(gtk::ResponseType::Cancel);
        let start = std::cell::RefCell::new(Some(start));
        dialog.connect_map_event(move |dialog, _| {
            let start = start.borrow_mut().take();
            if start.is_some_and(|start| !start.opened()) {
                dialog.response(gtk::ResponseType::Cancel);
            }
            gtk::glib::Propagation::Proceed
        });
        // GTK maps Escape/close to DeleteEvent. Only the destructive button's response accepts.
        let confirmed = dialog.run() == gtk::ResponseType::Accept;
        // SAFETY: the modal run has ended, and this owned dialog is never accessed again. GTK requires
        // explicit destruction after run(); requesting close would only queue another delete event.
        unsafe { dialog.destroy() };
        confirmed
    }
}
