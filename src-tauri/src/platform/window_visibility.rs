//! Window-server observations and their explicitly unobserved fallback, independent of call state.
use sidevoice_desktop_core::call_controls::{self as logic, Rect};
use tauri::{Runtime, WebviewWindow};

#[cfg_attr(target_os = "macos", path = "window_visibility/macos.rs")]
#[cfg_attr(target_os = "windows", path = "window_visibility/windows.rs")]
#[cfg_attr(target_os = "linux", path = "window_visibility/linux.rs")]
mod imp;
pub use imp::NativeVisibility;

pub trait WindowVisibility {
    /// Observe on the main thread with no application mutex held. Stage is in CSS pixels relative to content.
    /// `None` fields mean unavailable, never proof of visibility or lack of coverage. The existing platform
    /// fallback remains separate so Wayland focus is not advertised as a workspace/occlusion observation.
    fn observe<R: Runtime>(window: Option<&WebviewWindow<R>>, stage: Option<Rect>) -> Visibility;
}

#[derive(Debug, Clone, Copy)]
pub struct Visibility {
    pub shown: Option<bool>,
    pub minimised: Option<bool>,
    pub on_screen: Option<bool>,
    pub uncovered: Option<f64>,
    // Existing behavior when workspace/occlusion cannot be measured; not an observation.
    fallback: bool,
}

impl Visibility {
    pub fn stage_on_screen(&self) -> bool {
        logic::stage_on_screen(
            self.shown.unwrap_or(false),
            self.minimised.unwrap_or(false),
            self.on_screen.unwrap_or(self.fallback),
            self.uncovered,
        )
    }
}

fn sample<R: Runtime>(
    window: Option<&WebviewWindow<R>>,
    screen: impl FnOnce(&WebviewWindow<R>) -> (Option<bool>, bool),
    coverage: impl FnOnce(&WebviewWindow<R>) -> Option<f64>,
) -> Visibility {
    let Some(w) = window else {
        return Visibility {
            shown: Some(false),
            minimised: Some(false),
            on_screen: Some(false),
            uncovered: None,
            fallback: false,
        };
    };
    let shown = w.is_visible().ok();
    let minimised = w.is_minimized().ok();
    let (on_screen, fallback) = screen(w);
    let uncovered = if shown == Some(true) && minimised != Some(true) && on_screen.unwrap_or(fallback) {
        coverage(w)
    } else {
        None
    };
    Visibility { shown, minimised, on_screen, uncovered, fallback }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::MockRuntime;

    #[test]
    fn missing_window_is_not_a_visible_stage() {
        let seen = NativeVisibility::observe::<MockRuntime>(None, None);
        assert_eq!(seen.shown, Some(false));
        assert!(!seen.stage_on_screen());
        assert_eq!(seen.uncovered, None);
    }

    #[test]
    fn unavailable_workspace_is_distinct_from_both_observed_states() {
        let mut seen =
            Visibility { shown: Some(true), minimised: Some(false), on_screen: None, uncovered: None, fallback: true };
        assert!(seen.stage_on_screen());
        assert_eq!(seen.on_screen, None);
        seen.fallback = false; // Wayland without focus: still not an observed workspace state.
        assert!(!seen.stage_on_screen());
        assert_eq!(seen.on_screen, None);
        seen.on_screen = Some(true);
        assert!(seen.stage_on_screen());
        seen.on_screen = Some(false); // DWM cloaked, or AppKit occluded.
        assert!(!seen.stage_on_screen());
    }

    #[test]
    fn coverage_unknown_is_not_zero_and_hidden_or_minimised_always_wins() {
        let mut seen = Visibility {
            shown: Some(true),
            minimised: Some(false),
            on_screen: Some(true),
            uncovered: None,
            fallback: true,
        };
        assert!(seen.stage_on_screen());
        seen.uncovered = Some(0.0);
        assert!(!seen.stage_on_screen());
        seen.uncovered = Some(1.0);
        seen.minimised = Some(true);
        assert!(!seen.stage_on_screen());
        seen.minimised = Some(false);
        seen.shown = Some(false);
        assert!(!seen.stage_on_screen());
    }
}
