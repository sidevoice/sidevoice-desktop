//! The webview switches this build and its native runtime implement. Probe pages are a separate feature.

#[cfg(any(test, sidevoice_debug_options))]
use sidevoice_desktop_core::debug_options::DebugOptions;
use sidevoice_desktop_core::debug_options::Switch;

#[cfg_attr(target_os = "macos", path = "webview_diagnostics/macos.rs")]
#[cfg_attr(target_os = "windows", path = "webview_diagnostics/windows.rs")]
#[cfg_attr(target_os = "linux", path = "webview_diagnostics/linux.rs")]
mod imp;
use imp::Os;

/// What differs per OS: which diagnostic setters its webview has, and applying them.
trait Native {
    /// The switches with native setters in this build/runtime; none where no binding is implemented.
    fn available() -> Vec<Switch> {
        Vec::new()
    }

    /// Schedule the options on this webview's UI thread, every enabled one already known to be available.
    /// Native implementations must check selector availability before using private APIs.
    #[cfg(sidevoice_debug_options)]
    fn apply(_window: &tauri::WebviewWindow, _options: DebugOptions) -> Result<(), String> {
        Err("webview diagnostics unavailable".into())
    }
}

/// Only switches with native setters in this build/runtime. Empty in releases and on Windows/Linux,
/// where no WebView2 or WebKitGTK diagnostic binding is implemented.
pub fn available() -> Vec<Switch> {
    Os::available()
}

/// Schedule the kept options on this webview's UI thread. Reject unsupported enabled options and surface
/// native dispatch errors.
#[cfg(sidevoice_debug_options)]
pub fn apply(window: &tauri::WebviewWindow, options: DebugOptions) -> Result<(), String> {
    let available = available();
    if Switch::ALL.into_iter().any(|switch| options.get(switch) && !available.contains(&switch)) {
        return Err("unsupported webview diagnostic switch".into());
    }
    Os::apply(window, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_implemented_switches_reach_the_page() {
        let available = available();
        assert!(available.iter().all(|switch| Switch::ALL.contains(switch)));
        let options = DebugOptions { inspectable: true, compositing_borders: true, repaint_counters: true };
        let state = options.state(&available);
        for switch in Switch::ALL {
            assert_eq!(state[switch.name()], available.contains(&switch));
        }
    }

    #[test]
    fn an_unimplemented_backend_advertises_no_switches() {
        struct Unsupported;
        impl Native for Unsupported {}
        assert!(Unsupported::available().is_empty());
        // This backend cannot touch a window: only its advertised state is consumed by production callers.
        assert_eq!(DebugOptions::default().state(&Unsupported::available())["available"], serde_json::json!([]));
    }
}
