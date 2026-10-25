//! The webview switches this build and its native runtime implement. Probe pages are a separate feature.

#[cfg(any(test, sidevoice_debug_options))]
use sidevoice_desktop_core::debug_options::DebugOptions;
use sidevoice_desktop_core::debug_options::Switch;

#[cfg(all(sidevoice_debug_options, target_os = "macos"))]
#[path = "webview_diagnostics/macos.rs"]
mod imp;
#[cfg(not(all(sidevoice_debug_options, target_os = "macos")))]
mod imp {
    pub struct Os;
    impl super::WebviewDiagnostics for Os {}
}
pub use imp::Os as NativeWebviewDiagnostics;

pub trait WebviewDiagnostics {
    /// Only switches with native setters in this build/runtime. Empty in releases and on Windows/Linux,
    /// where no WebView2 or WebKitGTK diagnostic binding is implemented.
    fn available() -> Vec<Switch> {
        Vec::new()
    }

    /// Schedule the kept options on this webview's UI thread. Reject unsupported enabled options and surface
    /// native dispatch errors. Native implementations must check selector availability before using private APIs.
    #[cfg(sidevoice_debug_options)]
    fn apply(_window: &tauri::WebviewWindow, _options: DebugOptions) -> Result<(), String> {
        Err("webview diagnostics unavailable".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_implemented_switches_reach_the_page() {
        let available = NativeWebviewDiagnostics::available();
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
        impl WebviewDiagnostics for Unsupported {}
        assert!(Unsupported::available().is_empty());
        // This backend cannot touch a window: only its advertised state is consumed by production callers.
        assert_eq!(DebugOptions::default().state(&Unsupported::available())["available"], serde_json::json!([]));
    }
}
