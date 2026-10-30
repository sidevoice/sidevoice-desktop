//! Window-server observations of the room window, independent of call state.
use sidevoice_desktop_core::call_controls::{self as logic, Rect};
use tauri::{Runtime, WebviewWindow};

#[cfg_attr(target_os = "macos", path = "window_visibility/macos.rs")]
#[cfg_attr(target_os = "windows", path = "window_visibility/windows.rs")]
#[cfg_attr(target_os = "linux", path = "window_visibility/linux.rs")]
mod imp;

/// What an OS can honestly observe about the room window beyond what Tauri reports everywhere. Both run on the main
/// thread with no application mutex held; `None` means this OS cannot tell, never proof either way.
trait Os {
    /// Whether any part of the window is on screen (not on another workspace, not occluded).
    fn on_screen<R: Runtime>(_: &WebviewWindow<R>) -> Option<bool> {
        None
    }

    /// The part of the stage (CSS pixels relative to content) no other app's window covers, from 0.0 to 1.0.
    fn uncovered<R: Runtime>(_: &WebviewWindow<R>, _: Option<Rect>) -> Option<f64> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Visibility {
    pub shown: Option<bool>,
    pub minimised: Option<bool>,
    pub on_screen: Option<bool>,
    pub uncovered: Option<f64>,
}

impl Visibility {
    /// Unknown counts as visible: an unobserved workspace or coverage never hides the stage by itself.
    pub fn stage_on_screen(&self) -> bool {
        logic::stage_on_screen(
            self.shown.unwrap_or(false),
            self.minimised.unwrap_or(false),
            self.on_screen.unwrap_or(true),
            self.uncovered,
        )
    }
}

/// Observe on the main thread with no application mutex held. Stage is in CSS pixels relative to content. A missing
/// window is not a visible stage.
pub fn observe<R: Runtime>(window: Option<&WebviewWindow<R>>, stage: Option<Rect>) -> Visibility {
    let Some(w) = window else {
        return Visibility { shown: Some(false), minimised: Some(false), on_screen: Some(false), uncovered: None };
    };
    let shown = w.is_visible().ok();
    let minimised = w.is_minimized().ok();
    let on_screen = imp::Native::on_screen(w);
    let uncovered = if shown == Some(true) && minimised != Some(true) && on_screen != Some(false) {
        imp::Native::uncovered(w, stage)
    } else {
        None
    };
    Visibility { shown, minimised, on_screen, uncovered }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::MockRuntime;

    #[test]
    fn missing_window_is_not_a_visible_stage() {
        let seen = observe::<MockRuntime>(None, None);
        assert_eq!(seen.shown, Some(false));
        assert!(!seen.stage_on_screen());
        assert_eq!(seen.uncovered, None);
    }

    #[test]
    fn unknown_workspace_counts_as_visible_but_observed_states_decide() {
        let mut seen = Visibility { shown: Some(true), minimised: Some(false), on_screen: None, uncovered: None };
        assert!(seen.stage_on_screen());
        seen.on_screen = Some(true);
        assert!(seen.stage_on_screen());
        seen.on_screen = Some(false); // DWM cloaked, AppKit occluded, or an unfocused Wayland window.
        assert!(!seen.stage_on_screen());
    }

    #[test]
    fn coverage_unknown_is_not_zero_and_hidden_or_minimised_always_wins() {
        let mut seen = Visibility { shown: Some(true), minimised: Some(false), on_screen: Some(true), uncovered: None };
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
