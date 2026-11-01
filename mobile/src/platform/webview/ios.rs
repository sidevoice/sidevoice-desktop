//! iOS: the web view is made to fill its parent view's bounds (wry creates it with the parent's `frame`, which is in
//! the parent's parent's coordinates), and the window and the view controller's view get the system background, so what
//! the page does not cover shows the system's background (light or dark with the device) instead of the window's unset
//! colour. Its scroll view cannot be pinch-zoomed: an app is not a document, and a zoomed-out page left the screen black
//! around it. The window is only there once the view is on screen, so this runs again after the page loads.
use super::Os;
use objc2::{msg_send, rc::Retained};
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

/// Keeps the page at its own scale: no pinch, no zoom range, no bounce back from one. WebKit sets the zoom range from
/// the page's viewport each time it loads, so this runs after each load too; the pinch recognizer is the part it leaves.
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
        let insets = web_view.safeAreaInsets();
        Some(format!(
            "window {}, view {}, web view {}, safe area top {} bottom {} left {} right {}",
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
