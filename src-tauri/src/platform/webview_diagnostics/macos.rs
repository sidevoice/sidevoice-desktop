use super::WebviewDiagnostics;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, sel};
use sidevoice_desktop_core::debug_options::{DebugOptions, Switch};

pub struct Os;

impl WebviewDiagnostics for Os {
    fn available() -> Vec<Switch> {
        let has = |name, selector| AnyClass::get(name).is_some_and(|class| class.instance_method(selector).is_some());
        [
            (Switch::Inspectable, has(c"WKWebView", sel!(setInspectable:))),
            (Switch::CompositingBorders, has(c"WKPreferences", sel!(_setCompositingBordersVisible:))),
            (Switch::RepaintCounters, has(c"WKPreferences", sel!(_setCompositingRepaintCountersVisible:))),
        ]
        .into_iter()
        .filter_map(|(switch, present)| present.then_some(switch))
        .collect()
    }

    fn apply(window: &tauri::WebviewWindow, options: DebugOptions) -> Result<(), String> {
        if Switch::ALL.into_iter().any(|switch| options.get(switch) && !Self::available().contains(&switch)) {
            return Err("unsupported webview diagnostic switch".into());
        }
        window
            .with_webview(move |webview| {
                let view = webview.inner() as *mut AnyObject;
                if view.is_null() {
                    return;
                }
                // SAFETY: the window's own WKWebView and its live preferences, on the main thread (`with_webview`); each setter
                // only where the system answers to it.
                unsafe {
                    let responds: bool = msg_send![view, respondsToSelector: sel!(setInspectable:)];
                    if responds {
                        let _: () = msg_send![view, setInspectable: Bool::new(options.inspectable)];
                    }
                    let configuration: *mut AnyObject = msg_send![view, configuration];
                    if configuration.is_null() {
                        return;
                    }
                    let preferences: *mut AnyObject = msg_send![configuration, preferences];
                    if preferences.is_null() {
                        return;
                    }
                    let responds: bool = msg_send![preferences, respondsToSelector: sel!(_setCompositingBordersVisible:)];
                    if responds {
                        let _: () =
                            msg_send![preferences, _setCompositingBordersVisible: Bool::new(options.compositing_borders)];
                    }
                    let responds: bool =
                        msg_send![preferences, respondsToSelector: sel!(_setCompositingRepaintCountersVisible:)];
                    if responds {
                        let _: () =
                            msg_send![preferences, _setCompositingRepaintCountersVisible: Bool::new(options.repaint_counters)];
                    }
                }
            })
            .map_err(|error| error.to_string())
    }
}
