use super::{Confirmation, ConfirmationKind, DialogStart};

pub struct NativeConfirmation;

fn cancelled(start: &DialogStart) -> bool {
    tokio::time::Instant::now() >= start.deadline || start.started.is_closed()
}

impl Confirmation for NativeConfirmation {
    fn show(kind: ConfirmationKind, start: DialogStart) -> bool {
        use objc2::{class, msg_send, runtime::AnyObject};
        use objc2_app_kit::NSModalPanelRunLoopMode;
        use objc2_foundation::{NSArray, NSRunLoop, NSString};
        use std::sync::{Arc, Mutex};

        // A callback delayed on the main queue must never show an already-cancelled request.
        if cancelled(&start) {
            return false;
        }
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
                let start = Arc::new(Mutex::new(Some(start)));
                let modal_start = Arc::clone(&start);
                let opened = block2::RcBlock::new(move || {
                    let start = modal_start.lock().unwrap().take();
                    if start.is_some_and(|start| !start.opened()) {
                        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                        let _: () = msg_send![app, abortModal];
                    }
                });
                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let _: () = msg_send![app, activateIgnoringOtherApps: true];
                // Activation can itself be delayed. Recheck before entering the modal session.
                let expired = start.lock().unwrap().as_ref().is_some_and(cancelled);
                if expired {
                    start.lock().unwrap().take();
                    let _: () = msg_send![alert, release];
                    return false;
                }
                let modes = NSArray::from_slice(&[NSModalPanelRunLoopMode]);
                // Runs on the UI thread only once AppKit enters its modal loop, even if the alert never
                // becomes key. A session that starts after cancellation is immediately aborted there.
                NSRunLoop::mainRunLoop().performInModes_block(&modes, &opened);
                let answer: isize = msg_send![alert, runModal];
                // Invalidate an unserviced block so it cannot abort a later, unrelated modal session.
                start.lock().unwrap().take();
                let _: () = msg_send![alert, release];
                answer == 1001 // NSAlertSecondButtonReturn
            }
        }
    }
}
