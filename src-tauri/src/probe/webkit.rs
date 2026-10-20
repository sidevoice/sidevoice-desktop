//! CI only (the `probe` feature, macOS): WebKit's feature flags, listed and switched on from the environment, to measure
//! what the layer-based SVG engine (and any other flag) does to the CPU an animated SVG costs (`macos svg-cpu`).
//!
//! - `SIDEVOICE_DEBUG_WEBKIT_FEATURES=1`: every feature WebKit has (`+[WKPreferences _features]`,
//!   `_experimentalFeatures`, `_internalDebugFeatures`) is logged as a `webkit-feature` line: its key, name, status,
//!   default and the value the room's webview has.
//! - `SIDEVOICE_DEBUG_WEBKIT_ENABLE=KEY,-KEY2,...`: those features are switched on (a leading `-` switches one off) on
//!   the `WKWebViewConfiguration` the room's webview is created with, before the page or its process exist.
//! - `SIDEVOICE_DEBUG_WEBKIT_LATE=1`: the same, but on the live webview after it exists, followed by a reload; says
//!   whether WebKit takes the flag up late.
//!
//! The private `_WKFeature` API is asked only of what answers to it.

use std::ffi::{c_char, CStr};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{class, msg_send, sel, MainThreadMarker};
use objc2_web_kit::WKWebViewConfiguration;
use tauri::WebviewWindow;

fn flag(name: &str) -> bool {
    crate::debugging() && std::env::var_os(name).is_some_and(|v| v == "1")
}

/// The features to switch: `(key, on)`.
fn wanted() -> Vec<(String, bool)> {
    if !crate::debugging() {
        return Vec::new();
    }
    let asked = std::env::var("SIDEVOICE_DEBUG_WEBKIT_ENABLE").unwrap_or_default();
    asked
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(|key| match key.strip_prefix('-') {
            Some(key) => (key.to_owned(), false),
            None => (key.to_owned(), true),
        })
        .collect()
}

unsafe fn responds(object: *mut AnyObject, selector: Sel) -> bool {
    !object.is_null() && msg_send![object, respondsToSelector: selector]
}

unsafe fn text(string: *mut AnyObject) -> String {
    if !responds(string, sel!(UTF8String)) {
        return String::new();
    }
    let chars: *const c_char = msg_send![string, UTF8String];
    if chars.is_null() {
        String::new()
    } else {
        CStr::from_ptr(chars).to_string_lossy().into_owned()
    }
}

/// Every `_WKFeature`, with the list it came from.
unsafe fn features() -> Vec<(&'static str, *mut AnyObject)> {
    let class = class!(WKPreferences);
    let mut all = Vec::new();
    for (kind, selector) in [
        ("feature", sel!(_features)),
        ("experimental", sel!(_experimentalFeatures)),
        ("internal", sel!(_internalDebugFeatures)),
    ] {
        let known: bool = msg_send![class, respondsToSelector: selector];
        if !known {
            crate::debug(&format!("webkit-features: WKPreferences does not answer to {kind} list"));
            continue;
        }
        let list: *mut AnyObject = msg_send![class, performSelector: selector];
        if list.is_null() {
            continue;
        }
        let count: usize = msg_send![list, count];
        for at in 0..count {
            let feature: *mut AnyObject = msg_send![list, objectAtIndex: at];
            all.push((kind, feature));
        }
    }
    all
}

unsafe fn key_of(feature: *mut AnyObject) -> String {
    let key: *mut AnyObject = msg_send![feature, key];
    text(key)
}

unsafe fn value(preferences: *mut AnyObject, feature: *mut AnyObject) -> String {
    if responds(preferences, sel!(_isEnabledForFeature:)) {
        let on: bool = msg_send![preferences, _isEnabledForFeature: feature];
        on.to_string()
    } else {
        "unknown".to_owned()
    }
}

unsafe fn list(preferences: *mut AnyObject, when: &str) {
    for (kind, feature) in features() {
        let name: *mut AnyObject = msg_send![feature, name];
        let status = if responds(feature, sel!(status)) {
            let status: isize = msg_send![feature, status];
            status.to_string()
        } else {
            "?".to_owned()
        };
        let default = if responds(feature, sel!(defaultValue)) {
            let on: bool = msg_send![feature, defaultValue];
            on.to_string()
        } else {
            "?".to_owned()
        };
        crate::debug(&format!(
            "webkit-feature when={when} list={kind} key={} status={status} default={default} value={} name={:?}",
            key_of(feature),
            value(preferences, feature),
            text(name)
        ));
    }
}

/// Switches the wanted features on `preferences`, and says what each is afterwards.
unsafe fn apply(preferences: *mut AnyObject, when: &str) {
    if !responds(preferences, sel!(_setEnabled:forFeature:)) {
        crate::debug("webkit-enable: WKPreferences does not answer to _setEnabled:forFeature:");
        return;
    }
    for (key, on) in wanted() {
        let mut found = false;
        for (kind, feature) in features() {
            if key_of(feature) != key {
                continue;
            }
            found = true;
            let before = value(preferences, feature);
            let _: () = msg_send![preferences, _setEnabled: Bool::new(on), forFeature: feature];
            crate::debug(&format!(
                "webkit-enable when={when} list={kind} key={key} wanted={on} before={before} after={}",
                value(preferences, feature)
            ));
        }
        if !found {
            crate::debug(&format!("webkit-enable when={when} key={key} not-found"));
        }
    }
}

/// The configuration the room's webview is created with, when the features are to be set before it exists.
pub fn configuration() -> Option<Retained<WKWebViewConfiguration>> {
    if flag("SIDEVOICE_DEBUG_WEBKIT_LATE") || !(flag("SIDEVOICE_DEBUG_WEBKIT_FEATURES") || !wanted().is_empty()) {
        return None;
    }
    let Some(main) = MainThreadMarker::new() else {
        crate::debug("webkit-enable: not on the main thread, the webview gets WebKit's own configuration");
        return None;
    };
    // SAFETY: created on the main thread (the marker).
    let config = unsafe { WKWebViewConfiguration::new(main) };
    // SAFETY: the main thread; every selector is asked of what answers to it.
    unsafe {
        let preferences: *mut AnyObject = msg_send![&*config, preferences];
        if flag("SIDEVOICE_DEBUG_WEBKIT_FEATURES") {
            list(preferences, "config");
        }
        apply(preferences, "config");
    }
    Some(config)
}

/// Once the window exists: says what its live webview has for the wanted features, and, with
/// `SIDEVOICE_DEBUG_WEBKIT_LATE=1`, switches them on there and reloads.
pub fn report(window: &WebviewWindow) {
    let late = flag("SIDEVOICE_DEBUG_WEBKIT_LATE");
    if !(late || flag("SIDEVOICE_DEBUG_WEBKIT_FEATURES") || !wanted().is_empty()) {
        return;
    }
    let sent = window.with_webview(move |webview| {
        let view = webview.inner() as *mut AnyObject;
        // SAFETY: the main thread (with_webview runs there); the webview is a WKWebView.
        unsafe {
            let config: *mut AnyObject = msg_send![view, configuration];
            let preferences: *mut AnyObject = msg_send![config, preferences];
            if late {
                if flag("SIDEVOICE_DEBUG_WEBKIT_FEATURES") {
                    list(preferences, "late-before");
                }
                apply(preferences, "late");
                let _: *mut AnyObject = msg_send![view, reload];
            }
            for (key, _) in wanted() {
                for (kind, feature) in features() {
                    if key_of(feature) == key {
                        crate::debug(&format!(
                            "webkit-live list={kind} key={key} value={}",
                            value(preferences, feature)
                        ));
                    }
                }
            }
        }
    });
    if let Err(error) = sent {
        crate::debug(&format!("webkit-live: with_webview failed: {error}"));
    }
}
