use super::{DialogStart, Native};

pub(super) struct Os;

impl Native for Os {
    /// The current locale's language code, as AppKit's own alerts read it; English when it has none (never the
    /// preferred languages).
    fn language() -> Option<String> {
        use objc2::{class, msg_send, runtime::AnyObject};
        // SAFETY: public Foundation getters; `UTF8String` stays valid while `code` lives, and is copied at once.
        unsafe {
            let locale: *mut AnyObject = msg_send![class!(NSLocale), currentLocale];
            let code: *mut AnyObject = msg_send![locale, languageCode];
            let utf8: *const libc::c_char = msg_send![code, UTF8String];
            if utf8.is_null() {
                Some("en".to_owned())
            } else {
                Some(std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
            }
        }
    }

    fn show(words: (&'static str, &'static str, &'static str, &'static str), start: DialogStart) -> bool {
        use objc2::{class, msg_send, runtime::AnyObject};
        use objc2_app_kit::NSModalPanelRunLoopMode;
        use objc2_foundation::{NSArray, NSRunLoop, NSString};
        use std::sync::{Arc, Mutex};

        // A callback delayed on the main queue must never show an already-cancelled request.
        if start.cancelled() {
            return false;
        }
        // NSAlert's first button is the default. The destructive button is deliberately second.
        unsafe {
            let alert: *mut AnyObject = msg_send![class!(NSAlert), new];
            if alert.is_null() {
                false
            } else {
                let (title_text, detail_text, cancel_text, delete_text) = words;
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
                let expired = start.lock().unwrap().as_ref().is_some_and(DialogStart::cancelled);
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
                // Explicitly remove the panel after a programmatic abort as well as a button response.
                let window: *mut AnyObject = msg_send![alert, window];
                let _: () = msg_send![window, orderOut: std::ptr::null::<AnyObject>()];
                // Invalidate an unserviced block so it cannot abort a later, unrelated modal session.
                start.lock().unwrap().take();
                let _: () = msg_send![alert, release];
                answer == 1001 // NSAlertSecondButtonReturn
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::confirmation::ConfirmationKind;

    #[test]
    fn an_expired_request_cancels_before_accessing_appkit() {
        let (started, _start) = tokio::sync::oneshot::channel();
        let start = DialogStart { deadline: tokio::time::Instant::now(), started };
        // Runs on a test thread: an expired request must return before any main-thread-only AppKit call.
        assert!(!Os::show(ConfirmationKind::DeleteAllData.words("en"), start));
    }
}
