use tauri::{AppHandle, Runtime};

use super::{Confirmation, ConfirmationKind};

pub struct NativeConfirmation;

impl Confirmation for NativeConfirmation {
    async fn confirm<R: Runtime>(app: &AppHandle<R>, kind: ConfirmationKind) -> bool {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        if app
            .run_on_main_thread(move || {
                use objc2::{class, msg_send, runtime::AnyObject};
                use objc2_foundation::NSString;
                // NSAlert's first button is the default. The destructive button is deliberately second.
                let confirmed = unsafe {
                    let alert: *mut AnyObject = msg_send![class!(NSAlert), new];
                    if alert.is_null() {
                        false
                    } else {
                        let locale: *mut AnyObject = msg_send![class!(NSLocale), currentLocale];
                        let code: *mut AnyObject = msg_send![locale, languageCode];
                        let utf8: *const libc::c_char = msg_send![code, UTF8String];
                        let language =
                            if utf8.is_null() { "en" } else { std::ffi::CStr::from_ptr(utf8).to_str().unwrap_or("en") };
                        let (title_text, detail_text, cancel_text, delete_text) = kind.words(language);
                        let title = NSString::from_str(title_text);
                        let detail = NSString::from_str(detail_text);
                        let cancel = NSString::from_str(cancel_text);
                        let delete = NSString::from_str(delete_text);
                        let _: () = msg_send![alert, setMessageText: &*title];
                        let _: () = msg_send![alert, setInformativeText: &*detail];
                        let _: *mut AnyObject = msg_send![alert, addButtonWithTitle: &*cancel];
                        let _: *mut AnyObject = msg_send![alert, addButtonWithTitle: &*delete];
                        let answer: isize = msg_send![alert, runModal];
                        let _: () = msg_send![alert, release];
                        answer == 1001 // NSAlertSecondButtonReturn
                    }
                };
                let _ = sender.send(confirmed);
            })
            .is_err()
        {
            return false;
        }
        tauri::async_runtime::spawn_blocking(move || receiver.recv().unwrap_or(false)).await.unwrap_or(false)
    }
}
