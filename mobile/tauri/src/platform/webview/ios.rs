//! iOS: the web view is made to fill its parent view's bounds (wry creates it with the parent's `frame`, which is in
//! the parent's parent's coordinates), and the window and the view controller's view get the system background, so what
//! the page does not cover shows the system's background (light or dark with the device) instead of the window's unset
//! colour. Its scroll view cannot be pinch-zoomed: an app is not a document, and a zoomed-out page left the screen black
//! around it. Its scroll view adds no safe-area inset of its own (the page owns the safe areas), or WebKit lays the page out
//! 96 points short of the screen and the strip below it shows black. The window is only there once the view is on screen, so
//! this runs again after the page loads.
use super::Os;
use block2::RcBlock;
use objc2::{msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::NSString;
use objc2_ui_kit::{
    UIColor, UIScrollView, UIScrollViewContentInsetAdjustmentBehavior, UIView, UIViewController,
};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::webview::PlatformWebview;

pub struct Native;

/// The web view and the view controller's view, as UIKit objects. `None` if Tauri gave none.
fn views(webview: &PlatformWebview) -> Option<(&UIView, &UIViewController)> {
    let web_view = webview.inner().cast::<UIView>();
    let controller = webview.view_controller().cast::<UIViewController>();
    // SAFETY: Tauri hands the window's live `WKWebView` (a `UIView`) and `UIViewController`, which stay alive while the
    // window does, and this is used on the main thread (`with_webview`), where UIKit objects are.
    unsafe { Some((web_view.as_ref()?, controller.as_ref()?)) }
}

/// A second line against zooming, not the lock: the real one is the page's viewport (`maximum-scale=1, user-scalable=no`,
/// which the web sets when it runs in this app), because WebKit takes `allowsUserScaling` and the zoom range from the
/// viewport at every layer-tree commit and rewrites whatever is set here. Still, the scroll view's pinch recognizer is
/// turned off, the one zoom scale set and the bounce removed, which holds until WebKit next rewrites them; this runs
/// again after each page load. Focus auto-zoom and double-tap are the viewport's too, not this function's.
fn lock_zoom(web_view: &UIView) {
    // SAFETY: `scrollView` is `WKWebView`'s `UIScrollView` property, and `web_view` is the window's `WKWebView`.
    let scroll_view: Option<Retained<UIScrollView>> = unsafe { msg_send![web_view, scrollView] };
    let Some(scroll_view) = scroll_view else {
        return;
    };
    // The page owns the safe areas (`env(safe-area-inset-*)`): the scroll view adds none of its own. With the default
    // (automatic) it adds the status bar's and the home indicator's, and WebKit lays the page out in what is left: 860 of
    // the 956 points of a Pro Max, a strip of the screen the page does not cover.
    scroll_view
        .setContentInsetAdjustmentBehavior(UIScrollViewContentInsetAdjustmentBehavior::Never);
    scroll_view.setMinimumZoomScale(1.0);
    scroll_view.setMaximumZoomScale(1.0);
    scroll_view.setZoomScale(1.0);
    scroll_view.setBouncesZoom(false);
    if let Some(pinch) = scroll_view.pinchGestureRecognizer() {
        pinch.setEnabled(false);
    }
}

/// Asks the page what it measures (the viewport, the document, the root element and the backgrounds) and logs the answer:
/// the web view's side of a layout the native side cannot see.
/// What `log_page`'s script answers that is logged: sizes in CSS pixels.
#[derive(serde::Deserialize)]
struct PageLayout {
    inner: Option<[f64; 2]>,
    visual: Option<[f64; 2]>,
    client: Option<[f64; 2]>,
    scroll: Option<f64>,
    screen: Option<[f64; 2]>,
}

fn log_page(web_view: &UIView) {
    const SCRIPT: &str = "(() => { const r = (e) => { if (!e) return null; const b = e.getBoundingClientRect(); \
        return [Math.round(b.top), Math.round(b.height), getComputedStyle(e).backgroundColor].join(' '); }; \
        return JSON.stringify({ inner: [innerWidth, innerHeight], visual: window.visualViewport && [visualViewport.width, \
        visualViewport.height], client: [document.documentElement.clientWidth, document.documentElement.clientHeight], \
        scroll: document.documentElement.scrollHeight, html: r(document.documentElement), body: r(document.body), \
        root: r(document.getElementById('root')), screen: [screen.width, screen.height] }); })()";
    let script = NSString::from_str(SCRIPT);
    let done = RcBlock::new(|result: *mut AnyObject, _error: *mut AnyObject| {
        // SAFETY: the result of a script that returns a string is an `NSString`, or null on an error.
        // Only the numbers are read, through a type: the page is script, and its other values (colours, text) stay out of the log.
        if let Some(result) = unsafe { result.cast::<NSString>().as_ref() } {
            match serde_json::from_str::<PageLayout>(&result.to_string()) {
                Ok(page) => tracing::info!(
                    inner = ?page.inner,
                    visual = ?page.visual,
                    client = ?page.client,
                    scroll = ?page.scroll,
                    screen = ?page.screen,
                    "the page's layout was measured"
                ),
                Err(_) => tracing::warn!("the page's layout could not be read"),
            }
        }
    });
    // SAFETY: `evaluateJavaScript:completionHandler:` is `WKWebView`'s; the block takes the result and the error.
    let _: () =
        unsafe { msg_send![web_view, evaluateJavaScript: &*script, completionHandler: &*done] };
}

/// Reloads allowed in `RELOAD_WINDOW` after a termination, so a page that is killed on every load does not loop.
const MAX_RELOADS: usize = 3;
const RELOAD_WINDOW: Duration = Duration::from_secs(60);
static RELOADS: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

/// Records a reload and says whether it is within the limit.
fn reload_allowed(reloads: &Mutex<Vec<Instant>>) -> bool {
    let now = Instant::now();
    let mut reloads = reloads
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reloads.retain(|at| now.duration_since(*at) < RELOAD_WINDOW);
    if reloads.len() >= MAX_RELOADS {
        return false;
    }
    reloads.push(now);
    true
}

impl Os for Native {
    fn fill_screen(webview: &PlatformWebview) {
        let Some((web_view, controller)) = views(webview) else {
            return;
        };
        lock_zoom(web_view);
        let ground = UIColor::systemBackgroundColor();
        if let Some(container) = controller.view() {
            web_view.setFrame(container.bounds());
            container.setBackgroundColor(Some(&ground));
            if let Some(window) = container.window() {
                window.setBackgroundColor(Some(&ground));
            }
        }
    }

    fn layout(webview: &PlatformWebview) -> Option<String> {
        let (web_view, controller) = views(webview)?;
        let container = controller.view()?;
        let window = container.window();
        let rect = |view: &UIView| {
            let frame = view.frame();
            format!(
                "{}x{} at {},{}",
                frame.size.width, frame.size.height, frame.origin.x, frame.origin.y
            )
        };
        log_page(web_view);
        let insets = web_view.safeAreaInsets();
        let screen = window.as_deref().map_or("none".to_owned(), |w| {
            let bounds = w.screen().bounds();
            format!("{}x{}", bounds.size.width, bounds.size.height)
        });
        // SAFETY: `scrollView` is `WKWebView`'s `UIScrollView` property, and `web_view` is the window's `WKWebView`.
        let scroll_view: Option<Retained<UIScrollView>> =
            unsafe { msg_send![web_view, scrollView] };
        let scroll = scroll_view.as_deref().map_or("none".to_owned(), |s| {
            let adjusted = s.adjustedContentInset();
            format!(
                "{} adjusted inset top {} bottom {}",
                rect(s),
                adjusted.top,
                adjusted.bottom
            )
        });
        Some(format!(
            "screen {screen}, window {}, view {}, web view {}, scroll view {scroll}, safe area top {} bottom {} left {} right {}",
            window.as_deref().map_or("none".to_owned(), |w| rect(w)),
            rect(&container),
            rect(web_view),
            insets.top,
            insets.bottom,
            insets.left,
            insets.right,
        ))
    }

    fn watch_termination(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
        builder.on_web_content_process_terminate(|webview| {
            tracing::warn!("the web view's content process was terminated by the system");
            if !reload_allowed(&RELOADS) {
                tracing::error!(
                    "the web view's content process keeps being terminated: not reloading it again"
                );
                return;
            }
            // WebKit leaves the page blank (black screen) and wry does not reload it: reloading starts the page again,
            // which is what lets it rejoin the room.
            if let Err(error) = webview.reload() {
                tracing::error!(%error, "the web view could not be reloaded after its content process was terminated");
            }
        })
    }
}
