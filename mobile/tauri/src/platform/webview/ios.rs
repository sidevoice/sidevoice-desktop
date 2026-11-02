//! iOS: the web view is made to fill its parent view's bounds (wry creates it with the parent's `frame`, which is in
//! the parent's parent's coordinates), and the window and the view controller's view get the system background, so what
//! the page does not cover shows the system's background (light or dark with the device) instead of the window's unset
//! colour. Its scroll view cannot be pinch-zoomed: an app is not a document, and a zoomed-out page left the screen black
//! around it. The window is only there once the view is on screen, so this runs again after the page loads.
use super::Os;
use block2::RcBlock;
use objc2::{msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::NSString;
use objc2_ui_kit::{UIColor, UIScrollView, UIView, UIViewController};
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

/// Stops the pinch: the scroll view's pinch recognizer is off, which is what blocks the gesture. The one zoom scale and
/// no bounce are set too, but WebKit rewrites the zoom range while it renders and resizes, so they are only a second
/// line; this runs again after each page load. It does not touch focus auto-zoom (the web's fields are 16px) or
/// double-tap.
fn lock_zoom(web_view: &UIView) {
    // SAFETY: `scrollView` is `WKWebView`'s `UIScrollView` property, and `web_view` is the window's `WKWebView`.
    let scroll_view: Option<Retained<UIScrollView>> = unsafe { msg_send![web_view, scrollView] };
    let Some(scroll_view) = scroll_view else {
        return;
    };
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
        if let Some(result) = unsafe { result.cast::<NSString>().as_ref() } {
            log::info!("layout page: {result}");
        }
    });
    // SAFETY: `evaluateJavaScript:completionHandler:` is `WKWebView`'s; the block takes the result and the error.
    let _: () = unsafe { msg_send![web_view, evaluateJavaScript: &*script, completionHandler: &*done] };
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
        let scroll_view: Option<Retained<UIScrollView>> = unsafe { msg_send![web_view, scrollView] };
        let scroll = scroll_view.as_deref().map_or("none".to_owned(), |s| {
            let adjusted = s.adjustedContentInset();
            format!("{} adjusted inset top {} bottom {}", rect(s), adjusted.top, adjusted.bottom)
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
}
