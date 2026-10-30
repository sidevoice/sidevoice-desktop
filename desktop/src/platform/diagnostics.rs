//! What the process lets CI and the developer options observe of it: how its exit runs, its memory, what the app's
//! own voice call holds, and the webview switches this build implements. Per-OS differences live in
//! `diagnostics/{macos,windows,linux}.rs`. Probe observations exist in probe builds only (src/probe.rs); RSS is
//! never relabelled as macOS's physical footprint.

#[cfg(any(test, sidevoice_debug_options))]
use sidevoice_desktop_core::debug_options::DebugOptions;
use sidevoice_desktop_core::debug_options::Switch;
#[cfg(any(test, feature = "probe"))]
use sidevoice_desktop_core::memory::Sample;
#[cfg(feature = "probe")]
use tauri::{AppHandle, Runtime};

#[cfg_attr(target_os = "macos", path = "diagnostics/macos.rs")]
#[cfg_attr(target_os = "windows", path = "diagnostics/windows.rs")]
#[cfg_attr(target_os = "linux", path = "diagnostics/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS; the defaults are what an OS without the observation answers.
trait Os {
    /// Register the exit handler. Hosts whose exit has no native destructors to observe register nothing.
    #[cfg(feature = "probe")]
    fn report_handlers_ran() {}

    /// The physical footprint (macOS `task_info`), in bytes; `None` where this OS has no such reading.
    #[cfg(any(test, feature = "probe"))]
    fn footprint() -> Option<u64> {
        None
    }

    /// What the process holds on the GPU (macOS Metal), in bytes; `None` where this OS has no such reading.
    #[cfg(any(test, feature = "probe"))]
    fn metal() -> Option<u64> {
        None
    }

    /// The GPU's mapped regions (macOS `vmmap`), in bytes; `None` where this OS has no such reading.
    #[cfg(any(test, feature = "probe"))]
    fn accelerator() -> Option<u64> {
        None
    }

    /// The switches with native setters in this build/runtime; none where no binding is implemented.
    fn webview_switches() -> Vec<Switch> {
        Vec::new()
    }

    /// Schedule the options on this webview's UI thread, every enabled one already known to be available.
    /// Native implementations must check selector availability before using private APIs.
    #[cfg(sidevoice_debug_options)]
    fn apply_webview_switches(_window: &tauri::WebviewWindow, _options: DebugOptions) -> Result<(), String> {
        Err("webview diagnostics unavailable".into())
    }
}

/// Registers, before any model is loaded, a handler that logs `exit handlers ran` when `exit()` reaches it. It runs
/// after the destructors of the native libraries linked in, so its line says they ran to their end. Hosts whose
/// exit has no such destructors to observe register nothing.
#[cfg(feature = "probe")]
pub fn report_exit_handlers() {
    Native::report_handlers_ran()
}

/// Sample this process, in bytes. Missing readings stay absent, including the macOS-only footprint,
/// Metal allocation and vmmap accelerator regions. The portable observation is resident memory.
#[cfg(any(test, feature = "probe"))]
pub fn memory() -> Sample {
    Sample {
        footprint: Native::footprint(),
        resident: resident(),
        metal: Native::metal(),
        accelerator: Native::accelerator(),
    }
}

/// Resident memory, through the maintained portable API.
#[cfg(any(test, feature = "probe"))]
fn resident() -> Option<u64> {
    let mut system = sysinfo::System::new();
    let pid = sysinfo::get_current_pid().ok()?;
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_memory(),
    );
    system.process(pid).and_then(|process| resident_bytes(process.memory()))
}

// A live process cannot have an empty resident set. In particular, sysinfo leaves its initial zero on
// Windows when GetProcessMemoryInfo fails. Preserve that as unavailable rather than a measured zero.
#[cfg(any(test, feature = "probe"))]
fn resident_bytes(bytes: u64) -> Option<u64> {
    (bytes > 0).then_some(bytes)
}

/// What the app's voice host holds now (`keeps=N lanes=M`), or `None` while no voice host is managed.
#[cfg(feature = "probe")]
pub fn voice_holds<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    use tauri::Manager;
    app.try_state::<crate::voice::VoiceState>().map(|state| state.held())
}

/// Only switches with native setters in this build/runtime. Empty in releases and on Windows/Linux,
/// where no WebView2 or WebKitGTK diagnostic binding is implemented.
pub fn webview_switches() -> Vec<Switch> {
    Native::webview_switches()
}

/// Schedule the kept options on this webview's UI thread. Reject unsupported enabled options and surface
/// native dispatch errors.
#[cfg(sidevoice_debug_options)]
pub fn apply_webview_switches(window: &tauri::WebviewWindow, options: DebugOptions) -> Result<(), String> {
    let available = webview_switches();
    if Switch::ALL.into_iter().any(|switch| options.get(switch) && !available.contains(&switch)) {
        return Err("unsupported webview diagnostic switch".into());
    }
    Native::apply_webview_switches(window, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "probe")]
    #[test]
    fn a_host_with_nothing_to_observe_registers_nothing() {
        struct Unobserved;
        impl Os for Unobserved {}
        Unobserved::report_handlers_ran();
    }

    #[test]
    fn a_failed_live_process_reading_is_unavailable_in_the_probe_line() {
        let sample = Sample { resident: resident_bytes(0), ..Sample::default() };
        assert_eq!(sample.resident, None);
        assert_eq!(sample.line("failed"), "memory phase=failed footprint=na resident=na metal=na accelerator=na");
        assert_eq!(resident_bytes(4096), Some(4096));
    }

    #[test]
    fn portable_observations_do_not_invent_footprint_or_gpu_usage() {
        struct Portable;
        impl Os for Portable {}
        assert_eq!(Portable::footprint(), None);
        assert_eq!(Portable::metal(), None);
        assert_eq!(Portable::accelerator(), None);
    }

    #[test]
    fn a_sample_keeps_measurements_separate_and_serializes_missing_values() {
        assert!(memory().resident.is_some_and(|bytes| bytes > 0));
        assert_eq!(
            Sample::default().line("unknown"),
            "memory phase=unknown footprint=na resident=na metal=na accelerator=na"
        );
    }

    #[cfg(feature = "probe")]
    #[test]
    fn an_app_without_a_voice_host_holds_nothing_to_report() {
        let app = tauri::test::mock_builder().build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
        assert_eq!(voice_holds(app.handle()), None);
    }

    #[test]
    fn only_implemented_switches_reach_the_page() {
        let available = webview_switches();
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
        impl Os for Unsupported {}
        assert!(Unsupported::webview_switches().is_empty());
        // This backend cannot touch a window: only its advertised state is consumed by production callers.
        assert_eq!(DebugOptions::default().state(&Unsupported::webview_switches())["available"], serde_json::json!([]));
    }
}
