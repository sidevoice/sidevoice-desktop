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
//! window's focus: it is lost to split screen, the notification shade and a permission dialog while the page is on screen.
use std::sync::Arc;

use sidevoice_shared::app::AppHost;
use tauri::{AppHandle, Manager, RunEvent};

#[cfg_attr(target_os = "android", path = "app_activity/android.rs")]
#[cfg_attr(target_os = "ios", path = "app_activity/ios.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// Reports to `listener` every time the app becomes active (`true`) or resigns active or enters the background
    /// (`false`), from the thread the system posts on, for the rest of the process.
    fn observe(listener: Arc<dyn Fn(bool) + Send + Sync>);

    /// Whether the app's run event says the page is shown (`true`) or hidden (`false`); `None` for any other event.
    fn shown_by_event(event: &RunEvent) -> Option<bool>;
}

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
