use gtk::prelude::*;

use super::{DialogStart, Native};

pub(super) struct Os;

impl Native for Os {
    fn show(words: (&'static str, &'static str, &'static str, &'static str), start: DialogStart) -> bool {
        // Tauri normally initializes GTK on its UI thread. Do not construct a widget if that prerequisite
        // or the display connection is missing: GTK would abort rather than return a recoverable error.
        if !gtk::is_initialized_main_thread() || gtk::gdk::Display::default().is_none() {
            return false;
        }
        let (title, detail, cancel, action) = words;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::confirmation::{ConfirmationKind, START_TIMEOUT};

    #[test]
    fn without_gtk_on_this_thread_it_cancels_without_constructing_a_widget() {
        assert!(!gtk::is_initialized_main_thread());
        let (started, _start) = tokio::sync::oneshot::channel();
        let start = DialogStart { deadline: tokio::time::Instant::now() + START_TIMEOUT, started };
        assert!(!Os::show(ConfirmationKind::DeleteAllData.words("en"), start));
    }
}
