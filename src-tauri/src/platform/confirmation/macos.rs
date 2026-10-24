use super::{Confirmation, ConfirmationKind, DialogStart};

pub struct NativeConfirmation;

impl Confirmation for NativeConfirmation {
    fn show(kind: ConfirmationKind, start: DialogStart) -> bool {
        use objc2::{class, msg_send, runtime::AnyObject};
        use objc2_app_kit::NSWindowDidBecomeKeyNotification;
        use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};
        use std::ptr::NonNull;
        use std::sync::Mutex;
        // NSAlert's first button is the default. The destructive button is deliberately second.
        unsafe {
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
                let window: *mut AnyObject = msg_send![alert, window];
                if window.is_null() {
                    let _: () = msg_send![alert, release];
                    return false;
                }
                let start = Mutex::new(Some(start));
                let opened = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
                    let start = start.lock().unwrap().take();
                    if start.is_some_and(|start| !start.opened()) {
                        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                        let _: () = msg_send![app, abortModal];
                    }
                });
                let center = NSNotificationCenter::defaultCenter();
                // This sendable block acknowledges only this alert window's native key notification. A nil
                // queue delivers synchronously on AppKit's UI thread, where abortModal cancels the late alert.
                let observer = center.addObserverForName_object_queue_usingBlock(
                    Some(NSWindowDidBecomeKeyNotification),
                    Some(&*window),
                    None,
                    &opened,
                );
                let answer: isize = msg_send![alert, runModal];
                // Stop observing before releasing the alert/window, including when no key notification ran.
                center.removeObserver((*observer).as_ref());
                let _: () = msg_send![alert, release];
                answer == 1001 // NSAlertSecondButtonReturn
            }
        }
    }
}
