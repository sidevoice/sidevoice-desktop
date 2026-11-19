//! Whether the app is active, as the system says it. The page can draw only while it is: the system suspends the
//! web view when the app resigns active (Control Center, Siri, an alert, the app switcher) and when it goes to the
//! background.
//!
//! iOS: the window's events do not say it: the app has no UIScene manifest, so tao turns only
//! `applicationWillResignActive` into `Suspended`, `Resumed` comes only from `applicationWillEnterForeground`, and
//! `applicationDidBecomeActive` is no event at all; a resign that does not leave the foreground is never followed by a
//! `Resumed`. The application's own notifications are the one source there.
//!
//! Android tells it through the activity's lifecycle, which the window's `Suspended` and `Resumed` carry. Never the
use std::sync::Arc;

use sidevoice_shared::app::AppHost;
use tauri::{AppHandle, Manager, RunEvent};

/// What differs per OS.
trait Os {
    /// Reports to `listener` every time the app becomes active (`true`) or resigns active or enters the background
    /// (`false`), from the thread the system posts on, for the rest of the process.
    fn observe(listener: Arc<dyn Fn(bool) + Send + Sync>);

    /// Whether the app's run event says the page is shown (`true`) or hidden (`false`); `None` for any other event.
    fn shown_by_event(event: &RunEvent) -> Option<bool>;
}

/// iOS: the application's own notifications are the one source.
#[cfg(target_os = "ios")]
mod ios {
    use std::ptr::NonNull;
    use std::sync::Arc;

    use block2::RcBlock;
    use objc2_foundation::{NSNotification, NSNotificationCenter};
    use objc2_ui_kit::{
        UIApplicationDidBecomeActiveNotification, UIApplicationDidEnterBackgroundNotification,
        UIApplicationWillResignActiveNotification,
    };

    pub struct Native;

    impl super::Os for Native {
        fn observe(listener: Arc<dyn Fn(bool) + Send + Sync>) {
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
                    NSNotificationCenter::defaultCenter()
                        .addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
                };
                // The center keeps the observer until it is removed, and it is not removed: it lives as long as the process.
                std::mem::forget(observer);
            }
        }

        fn shown_by_event(_event: &super::RunEvent) -> Option<bool> {
            None
        }
    }
}

/// Android: the activity's lifecycle reaches the app as the window's `Suspended` and `Resumed`.
#[cfg(target_os = "android")]
mod android {
    use std::sync::Arc;

    pub struct Native;

    impl super::Os for Native {
        fn observe(_listener: Arc<dyn Fn(bool) + Send + Sync>) {}

        fn shown_by_event(event: &super::RunEvent) -> Option<bool> {
            match event {
                super::RunEvent::WindowEvent {
                    event: tauri::WindowEvent::Suspended,
                    ..
                } => Some(false),
                super::RunEvent::WindowEvent {
                    event: tauri::WindowEvent::Resumed,
                    ..
                } => Some(true),
                _ => None,
            }
        }
    }
}

#[cfg(target_os = "android")]
use android::Native;
#[cfg(target_os = "ios")]
use ios::Native;

/// From now on the host's page is shown only while the app is active, where the system says it by notifications.
pub fn observe(handle: &AppHandle) {
    let app = handle.clone();
    Native::observe(Arc::new(move |active| set_page_shown(&app, active)));
}

/// The same, for the systems that say it by the app's run events: call it with every event.
pub fn follow(app: &AppHandle, event: &RunEvent) {
    if let Some(shown) = Native::shown_by_event(event) {
        set_page_shown(app, shown);
    }
}

fn set_page_shown(app: &AppHandle, shown: bool) {
    if let Some(host) = app.try_state::<AppHost>() {
        host.set_page_shown(shown);
    }
}
