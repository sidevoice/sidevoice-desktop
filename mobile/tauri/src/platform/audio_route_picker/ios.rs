//! iOS: AVKit's `AVRoutePickerView` is the only way an app shows the system's output sheet, and it has no call to open it:
//! its own button is the control. The app keeps one of these views, hidden and without size, in the web view's window, and
//! sends its button the tap the page's button stands for. The sheet is the system's, over the app, and the app's audio
//! session (category, mode, options) is not touched. If a later iOS changes the view's inner button, the sheet does not
//! open and the open is refused (`audio-route-picker-failed`).
//!
//! AVKit is not linked by the Xcode project (the committed project is the Tauri CLI's output); its bundle is loaded when
//! the page first asks, which is a public API and needs nothing in the project.
use std::cell::RefCell;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyClass;
use objc2::{msg_send, sel};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSBundle, NSString};
use objc2_ui_kit::UIView;
use tauri::webview::PlatformWebview;

use super::Os;

pub struct Native;

/// `UIControlEventTouchUpInside`.
const TOUCH_UP_INSIDE: usize = 1 << 6;

thread_local! {
    /// The hidden picker view, on the main thread, kept for the next open.
    static PICKER: RefCell<Option<Retained<UIView>>> = const { RefCell::new(None) };
}

fn failed(why: &str) -> String {
    log::warn!("the output route picker did not open: {why}");
    "audio-route-picker-failed".into()
}

/// The picker view class, loading AVKit's bundle when it is not loaded yet.
fn picker_class() -> Option<&'static AnyClass> {
    if let Some(class) = AnyClass::get(c"AVRoutePickerView") {
        return Some(class);
    }
    let path = NSString::from_str("/System/Library/Frameworks/AVKit.framework");
    // SAFETY: loading a system framework's bundle.
    let loaded = NSBundle::bundleWithPath(&path).is_some_and(|bundle| unsafe { bundle.load() });
    if !loaded {
        return None;
    }
    AnyClass::get(c"AVRoutePickerView")
}

impl Os for Native {
    const AVAILABLE: bool = true;

    fn open(webview: &PlatformWebview) -> Result<(), String> {
        let web_view = webview.inner().cast::<UIView>();
        // SAFETY: Tauri hands the window's live `WKWebView` (a `UIView`) and this runs on the main thread (`with_webview`).
        let web_view = unsafe { web_view.as_ref() }.ok_or_else(|| failed("no web view"))?;
        PICKER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let class = picker_class().ok_or_else(|| failed("AVKit is not there"))?;
                // SAFETY: `AVRoutePickerView` is a `UIView` on iOS; `alloc` and `initWithFrame:` as for any view.
                let view: Retained<UIView> = unsafe {
                    let blank: Allocated<UIView> = msg_send![class, alloc];
                    msg_send![blank, initWithFrame: CGRect::ZERO]
                };
                view.setHidden(true);
                *slot = Some(view);
            }
            let view = slot.as_ref().ok_or_else(|| failed("no picker view"))?;
            // In the window the page is in: added again when the window's view changed.
            let parent = view.superview();
            if parent
                .as_deref()
                .map(|parent| std::ptr::eq(parent, web_view))
                != Some(true)
            {
                web_view.addSubview(view);
            }
            // SAFETY: messages to views of the picker's own; `sendActionsForControlEvents:` only to the one that takes it.
            let pressed = unsafe {
                view.subviews().iter().any(|inner| {
                    let takes: bool =
                        msg_send![&*inner, respondsToSelector: sel!(sendActionsForControlEvents:)];
                    if takes {
                        let _: () =
                            msg_send![&*inner, sendActionsForControlEvents: TOUCH_UP_INSIDE];
                    }
                    takes
                })
            };
            if pressed {
                Ok(())
            } else {
                Err(failed("the picker has no button"))
            }
        })
    }
}
