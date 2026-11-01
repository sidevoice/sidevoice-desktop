//! iOS: the web view's scroll view adjusts its content inset by the safe area (`contentInsetAdjustmentBehavior`,
//! automatic by default), which shrinks the page's viewport by the status bar and the home indicator. Setting it to
//! `never` makes the page's viewport the whole screen, with the insets reported through `env(safe-area-inset-*)`, which
//! the interface pads with.
use super::Os;
use objc2::{msg_send, runtime::AnyObject};
use tauri::webview::PlatformWebview;

pub struct Native;

/// `UIScrollViewContentInsetAdjustmentBehavior.never`.
const CONTENT_INSET_ADJUSTMENT_NEVER: isize = 2;

impl Os for Native {
    fn cover_screen(webview: &PlatformWebview) {
        let web_view = webview.inner().cast::<AnyObject>();
        if web_view.is_null() {
            return;
        }
        // SAFETY: `inner` is the window's live `WKWebView`, and this runs on the main thread (`with_webview`), where UIKit
        // objects are used. `scrollView` is a `UIScrollView` property of it, and `contentInsetAdjustmentBehavior` takes
        // the `NSInteger` enum above.
        unsafe {
            let scroll_view: *mut AnyObject = msg_send![web_view, scrollView];
            if !scroll_view.is_null() {
                let _: () = msg_send![
                    scroll_view,
                    setContentInsetAdjustmentBehavior: CONTENT_INSET_ADJUSTMENT_NEVER
                ];
            }
        }
    }
}
