use gtk::prelude::*;
use tauri::{AppHandle, Runtime};

use super::{Confirmation, ConfirmationKind};

pub struct NativeConfirmation;

impl Confirmation for NativeConfirmation {
    async fn confirm<R: Runtime>(app: &AppHandle<R>, kind: ConfirmationKind) -> bool {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        if app
            .run_on_main_thread(move || {
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
                // GTK maps Escape/close to DeleteEvent. Only the destructive button's response accepts.
                let confirmed = dialog.run() == gtk::ResponseType::Accept;
                dialog.close();
                let _ = sender.send(confirmed);
            })
            .is_err()
        {
            return false;
        }
        tauri::async_runtime::spawn_blocking(move || receiver.recv().unwrap_or(false)).await.unwrap_or(false)
    }
}
