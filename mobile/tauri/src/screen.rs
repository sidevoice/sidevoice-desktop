//! The screen stays on while the app needs it, whoever needs it: a model download (sidevoice-desktop#361: the system
//! suspends the app, and the download in it, soon after the screen turns off) and a call whose person asked for it
//! (sidevoice-desktop#384: `host.presence.screenAwake`). The OS-specific part is the plugin's; this owns the reasons, so
//! that one letting go does not turn the screen off under the other.
use std::sync::{Arc, Mutex};

use sidevoice_mobile_plugin::{keep_screen_on, ScreenError};
use sidevoice_shared::app::DownloadActivity;
use tauri::AppHandle;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct Reasons {
    download: bool,
    call: bool,
}

type Apply = Box<dyn Fn(bool) -> Result<(), ScreenError> + Send + Sync>;

/// Keeps the screen on for as long as any reason holds it.
pub struct Screen {
    reasons: Mutex<Reasons>,
    apply: Apply,
}

impl Screen {
    pub fn new(app: AppHandle) -> Self {
        Self::with(Box::new(move |on| keep_screen_on(&app, on)))
    }

    fn with(apply: Apply) -> Self {
        Self {
            reasons: Mutex::new(Reasons::default()),
            apply,
        }
    }

    /// The call's reason: the person asked for the screen to stay on during the call.
    pub fn call(&self, on: bool) -> Result<(), ScreenError> {
        self.set(|reasons| reasons.call = on)
    }

    fn set(&self, change: impl FnOnce(&mut Reasons)) -> Result<(), ScreenError> {
        // Held across the OS call, so that two changes reach the OS in the order they were made.
        let mut reasons = self.reasons.lock().unwrap();
        let before = *reasons;
        change(&mut reasons);
        let (was, is) = (
            before.download || before.call,
            reasons.download || reasons.call,
        );
        if was == is {
            return Ok(());
        }
        (self.apply)(is)
    }
}

/// The host's download activity, as one reason for the screen.
pub struct DownloadScreen(pub Arc<Screen>);

impl DownloadScreen {
    fn set(&self, on: bool) {
        if let Err(error) = self.0.set(|reasons| reasons.download = on) {
            log::warn!(
                "screen: could not {} the screen on: {error}",
                if on { "keep" } else { "release" }
            );
        }
    }
}

impl DownloadActivity for DownloadScreen {
    fn started(&self) {
        self.set(true);
    }

    fn ended(&self) {
        self.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded() -> (Screen, Arc<Mutex<Vec<bool>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let screen = Screen::with(Box::new(move |on| {
            seen.lock().unwrap().push(on);
            Ok(())
        }));
        (screen, calls)
    }

    #[test]
    fn a_call_keeps_the_screen_on_and_lets_it_go() {
        let (screen, calls) = recorded();
        screen.call(true).unwrap();
        screen.call(true).unwrap();
        screen.call(false).unwrap();
        assert_eq!(*calls.lock().unwrap(), [true, false]);
    }

    #[test]
    fn the_screen_is_let_go_only_when_no_reason_is_left() {
        let (screen, calls) = recorded();
        let downloads = DownloadScreen(Arc::new(screen));
        downloads.started();
        downloads.0.call(true).unwrap();
        downloads.ended();
        assert_eq!(*calls.lock().unwrap(), [true], "the call still needs it");
        downloads.0.call(false).unwrap();
        assert_eq!(*calls.lock().unwrap(), [true, false]);
    }
}
