//! Whether the app is active, as the system says it (iOS only). The page can draw only while it is: the system suspends the
//! web view when the app resigns active (Control Center, Siri, an alert, the app switcher) and when it goes to the
//! background. The window's events do not say it on iOS: the app has no UIScene manifest, so tao turns only
//! `applicationWillResignActive` into `Suspended`, `Resumed` comes only from `applicationWillEnterForeground`, and
//! `applicationDidBecomeActive` is no event at all; a resign that does not leave the foreground is never followed by a
//! `Resumed`. The application's own notifications are the one source here.
//!
//! Android tells it through the activity's lifecycle, which the window's `Suspended` and `Resumed` carry (`lib.rs`).
use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use objc2_foundation::{NSNotification, NSNotificationCenter};
use objc2_ui_kit::{
    UIApplicationDidBecomeActiveNotification, UIApplicationDidEnterBackgroundNotification,
    UIApplicationWillResignActiveNotification,
};

/// Reports to `listener` every time the app becomes active (`true`) or resigns active or enters the background
/// (`false`), from the thread the system posts on, for the rest of the process.
pub fn observe(listener: Arc<dyn Fn(bool) + Send + Sync>) {
    // SAFETY: constants of UIKit, linked by the app.
    let names = unsafe {
        [
            (UIApplicationDidBecomeActiveNotification, true),
            (UIApplicationWillResignActiveNotification, false),
            (UIApplicationDidEnterBackgroundNotification, false),
        ]
    };
    for (name, active) in names {
        let listener = Arc::clone(&listener);
        let block = RcBlock::new(move |_: NonNull<NSNotification>| listener(active));
        // SAFETY: no object to filter by, no queue (the block runs on the thread that posts, and only hands the state on);
        // the block is Send + Sync (a listener is) and the center retains it.
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
    }
}
