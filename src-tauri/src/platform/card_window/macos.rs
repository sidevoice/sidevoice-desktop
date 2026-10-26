use super::{window_options, CardSpec, CardWindow};
use crate::debug;
use tauri::{AppHandle, LogicalSize, Manager, WebviewUrl, WebviewWindow};

pub struct NativeCardWindow;
tauri_nspanel::tauri_panel! {
    panel!(CallControlsPanel {
        config: {
            can_become_key_window: false,
            // It never becomes key (above), yet says it is: WebKit sets the page's CSS cursor only for a key window
            // (#139, [`hover_while_inactive`]). AppKit still sends the keyboard to the real key window, the person's app.
            is_key_window: true,
            can_become_main_window: false,
            is_floating_panel: true
        }
    })
}

impl CardWindow for NativeCardWindow {
    fn configure(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
        builder.plugin(tauri_nspanel::init())
    }
    fn create(app: &AppHandle, spec: CardSpec) -> tauri::Result<()> {
        use tauri_nspanel::{CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};
        PanelBuilder::<_, CallControlsPanel>::new(app, spec.label)
            .url(WebviewUrl::App(spec.page.into()))
            .size(tauri::Size::Logical(LogicalSize::new(spec.size.width, spec.size.height)))
            .level(PanelLevel::Status)
            .collection_behavior(
                CollectionBehavior::new().can_join_all_spaces().full_screen_auxiliary().stationary().ignores_cycle(),
            )
            .add_style_mask(StyleMask::empty().nonactivating_panel())
            .hides_on_deactivate(false)
            .has_shadow(false)
            .transparent(true)
            .with_window(move |w| window_options(w, spec.script))
            .build()?;
        // No `.no_activate(true)` above: it parks the app at `Prohibited` while the panel is built, and macOS does not
        // bring the Dock icon back when the policy returns to `Regular` (#117). The card is created before the room is
        // shown and the app activated (lib.rs setup), so there is no focus for it to steal.
        prevent_activation(app, spec.label);
        Ok(())
    }
    fn set_visible(app: &AppHandle, label: &str, visible: bool) {
        use tauri_nspanel::ManagerExt;
        if let Ok(panel) = app.get_webview_panel(label) {
            if visible {
                if let Some(w) = app.get_webview_window(label) {
                    hover_while_inactive(&w);
                }
                panel.show();
                debug(&format!("call-controls panel {}", describe(panel.as_panel())));
            } else {
                panel.hide();
            }
        }
    }
    fn app_active() -> String {
        use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
        let active: bool = unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            msg_send![app, isActive]
        };
        // NSApplicationActivationPolicyRegular is 0: the Dock icon, Dock Quit and Cmd+Tab (the card must not change it).
        let policy: isize = unsafe {
            let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            msg_send![app, activationPolicy]
        };
        format!("app_active={active} regular={}", policy == 0)
    }
}

/// What the panel is, for CI's log (SIDEVOICE_DEBUG=1): how it was asked to behave, read back from AppKit.
fn describe(panel: &tauri_nspanel::objc2_app_kit::NSPanel) -> String {
    use tauri_nspanel::objc2::{class, msg_send, runtime::AnyObject};
    // NSWindowStyleMaskNonactivatingPanel, NSWindowCollectionBehaviorCanJoinAllSpaces / FullScreenAuxiliary.
    const NONACTIVATING: usize = 1 << 7;
    const ALL_SPACES: usize = 1 << 0;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    let (mask, level, behavior, key, visible, number): (usize, isize, usize, bool, bool, isize) = unsafe {
        (
            msg_send![panel, styleMask],
            msg_send![panel, level],
            msg_send![panel, collectionBehavior],
            // AppKit's own key window: the panel answers `isKeyWindow` with yes for WebKit's sake (#139).
            {
                let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let key: *mut AnyObject = msg_send![app, keyWindow];
                std::ptr::eq(key.cast_const().cast(), panel)
            },
            msg_send![panel, isVisible],
            msg_send![panel, windowNumber],
        )
    };
    format!(
        "nonactivating={} level={level} all_spaces={} fullscreen_auxiliary={} key={key} visible={visible} window={number} pid={} {}",
        mask & NONACTIVATING != 0,
        behavior & ALL_SPACES != 0,
        behavior & FULL_SCREEN_AUXILIARY != 0,
        std::process::id(),
        NativeCardWindow::app_active()
    )
}

/// macOS: tells the window server, too, that the card never activates the app. NSPanel does so only when it is created
/// with the non-activating style; tauri-nspanel adds that style to an existing window, so AppKit treats the panel as
/// non-activating while the window server does not, and a drag on the card made the app frontmost (focus taken from
/// the person's app, CI's card step). `_setPreventsActivation:` is what NSPanel itself calls at creation:
/// https://philz.blog/nspanel-nonactivating-style-mask-flag/
fn prevent_activation(app: &AppHandle, label: &str) {
    use tauri_nspanel::objc2::{msg_send, runtime::Bool, sel};
    use tauri_nspanel::ManagerExt;
    let Ok(panel) = app.get_webview_panel(label) else { return };
    let panel = panel.as_panel();
    // SAFETY: main thread (setup); a private NSWindow method, called only when the window answers to it.
    let applied = unsafe {
        let responds: bool = msg_send![panel, respondsToSelector: sel!(_setPreventsActivation:)];
        if responds {
            let _: () = msg_send![panel, _setPreventsActivation: Bool::YES];
        }
        responds
    };
    debug(&format!("call-controls prevents activation: {applied}"));
}

/// macOS: hover on a card that is never key while the app is inactive (#139). WebKit tracks the mouse with a tracking
/// area that is active only in the key window (`NSTrackingActiveInKeyWindow`, WebKit's `WebViewImpl.mm`
/// `trackingAreaOptions()`), so the card's page never saw the pointer move: no `:hover`, no `mouseenter`. That area
/// is replaced by the same one active always, which AppKit delivers to inactive apps too; WebKit only re-creates it
/// when the system's scroller style changes, so it is replaced again each time the card shows. Idempotent.
fn hover_while_inactive(w: &WebviewWindow) {
    use tauri_nspanel::objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
    use tauri_nspanel::NSRect;
    // NSTrackingActiveInKeyWindow, NSTrackingActiveAlways.
    const IN_KEY_WINDOW: usize = 0x20;
    const ALWAYS: usize = 0x80;
    let _ = w.with_webview(|webview| {
        let view = webview.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        // SAFETY: the window's own WKWebView, on the main thread (`with_webview`); the areas are kept retained while
        // the view's own list changes.
        let replaced = unsafe {
            let list: Option<Retained<AnyObject>> = msg_send![view, trackingAreas];
            let areas: Vec<Retained<AnyObject>> = list
                .map(|list| {
                    let count: usize = msg_send![&*list, count];
                    (0..count).map(|i| msg_send![&*list, objectAtIndex: i]).collect()
                })
                .unwrap_or_default();
            let mut replaced = 0;
            for area in areas {
                let options: usize = msg_send![&*area, options];
                if options & IN_KEY_WINDOW == 0 {
                    continue;
                }
                let rect: NSRect = msg_send![&*area, rect];
                let owner: *mut AnyObject = msg_send![&*area, owner];
                let user_info: *mut AnyObject = msg_send![&*area, userInfo];
                let alloc: *mut AnyObject = msg_send![class!(NSTrackingArea), alloc];
                let always: *mut AnyObject = msg_send![
                    alloc,
                    initWithRect: rect,
                    options: (options & !IN_KEY_WINDOW) | ALWAYS,
                    owner: owner,
                    userInfo: user_info
                ];
                let Some(always) = Retained::from_raw(always) else { continue };
                let _: () = msg_send![view, removeTrackingArea: &*area];
                let _: () = msg_send![view, addTrackingArea: &*always];
                replaced += 1;
            }
            replaced
        };
        debug(&format!("call-controls tracks the pointer while inactive: replaced={replaced}"));
    });
}
