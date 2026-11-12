//! iOS: AVKit's `AVRoutePickerView` is the only way an app shows the system's output sheet, and it has no call to open it:
//! its own button is the control. The app keeps one of these views, hidden and without size, in the web view, and sends its
//! button the tap the page's button stands for. The sheet is the system's, over the app, and the app's audio session
//! (category, mode, options) is not touched. AVKit is linked by the project (`bundle > iOS > frameworks`); `objc2-av-kit` only
//! binds macOS's view, so the class is declared here, over `UIView`, which is what it is on iOS.
//!
//! The view does not say whether its sheet came up (and the window shows nothing of it: the sheet is the system's), so
//! `open` is done once the tap is delivered to the button. A failure is the view not taking the tap: no button, or one that
//! opens a menu.
use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{extern_class, msg_send, sel, MainThreadMarker};
use objc2_avf_audio::{AVAudioSession, AVAudioSessionRouteChangeNotification};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSNotification, NSNotificationCenter};
use objc2_ui_kit::{UIResponder, UIView};
use tauri::webview::PlatformWebview;

use super::{Changed, Os};

pub struct Native;

extern_class!(
    /// AVKit's route picker view (`AVRoutePickerView`), a `UIView` on iOS.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[name = "AVRoutePickerView"]
    struct RoutePickerView;
);

/// `UIControlEventTouchUpInside`.
const TOUCH_UP_INSIDE: usize = 1 << 6;

thread_local! {
    /// The hidden picker view, on the main thread, kept for the next open.
    static PICKER: RefCell<Option<Retained<RoutePickerView>>> = const { RefCell::new(None) };
}

fn failed(why: &str) -> String {
    log::warn!("the output route picker did not open: {why}");
    "audio-route-picker-failed".into()
}

/// The web view, from what Tauri hands over (the window's live `WKWebView`, a `UIView`, on the main thread).
fn view(webview: &PlatformWebview) -> Option<&UIView> {
    // SAFETY: as said; used on the main thread (`with_webview`).
    unsafe { webview.inner().cast::<UIView>().as_ref() }
}

impl Os for Native {
    const AVAILABLE: bool = true;

    fn open(webview: &PlatformWebview) -> Result<(), String> {
        let web_view = view(webview).ok_or_else(|| failed("no web view"))?;
        let main = MainThreadMarker::new().ok_or_else(|| failed("not on the main thread"))?;
        PICKER.with(|slot| {
            let mut slot = slot.borrow_mut();
            let picker = slot.get_or_insert_with(|| {
                // SAFETY: `initWithFrame:` as for any `UIView`, on the main thread.
                let picker: Retained<RoutePickerView> = unsafe {
                    msg_send![main.alloc::<RoutePickerView>(), initWithFrame: CGRect::ZERO]
                };
                picker.setHidden(true);
                picker
            });
            // In the window the page is in: added again when the web view changed.
            let parent = picker.superview();
            if parent
                .as_deref()
                .map(|parent| std::ptr::eq(parent, web_view))
                != Some(true)
            {
                web_view.addSubview(picker);
            }
            // The picker's button, a `UIButton` inside it: the one that takes a tap.
            let taps = |inner: &AnyObject| -> bool {
                // SAFETY: `respondsToSelector:` on a live view.
                unsafe { msg_send![inner, respondsToSelector: sel!(sendActionsForControlEvents:)] }
            };
            if let Some(inner) = picker.subviews().iter().find(|inner| taps(inner)) {
                // A button that opens a menu on a tap would not open the sheet on this event.
                // SAFETY: the same, then a message it answers to.
                let menu = unsafe {
                    let shows: bool =
                        msg_send![&*inner, respondsToSelector: sel!(showsMenuAsPrimaryAction)];
                    shows && msg_send![&*inner, showsMenuAsPrimaryAction]
                };
                if menu {
                    return Err(failed("the picker's button opens a menu"));
                }
                // SAFETY: `sendActionsForControlEvents:` takes the event mask, on a control that answered to it.
                let _: () =
                    unsafe { msg_send![&*inner, sendActionsForControlEvents: TOUCH_UP_INSIDE] };
                return Ok(());
            }
            Err(failed("the picker has no button"))
        })
    }

    fn current() -> Option<String> {
        // SAFETY: read-only messages to the shared session, which answers from any thread.
        let outputs = unsafe { AVAudioSession::sharedInstance().currentRoute().outputs() };
        // SAFETY: a read of a live port description.
        let name = unsafe { outputs.iter().next()?.portName().to_string() };
        let name = name.trim();
        (!name.is_empty()).then(|| name.to_owned())
    }

    fn observe(changed: Changed) -> Result<(), String> {
        // SAFETY: a constant of AVFAudio, linked by the app.
        let Some(name) = (unsafe { AVAudioSessionRouteChangeNotification }) else {
            return Err("audio-route-changes-unavailable".into());
        };
        let block = RcBlock::new(move |_: NonNull<NSNotification>| changed());
        // SAFETY: no object to filter by, no queue (the block runs on the thread that posts, and only hands the event on); the
        // block is Send + Sync (`Changed` is) and the center retains it.
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                None,
                &block,
            )
        };
        // The center keeps the observer until it is removed, and it is not removed: it lives as long as the process.
        std::mem::forget(observer);
        Ok(())
    }
}
