//! The screen stays on while the app needs it, whoever needs it: a model download (sidevoice-desktop#361: the system
//! suspends the app, and the download in it, soon after the screen turns off) and a call whose person asked for it
//! (sidevoice-desktop#384: `host.presence.screenAwake`). The OS-specific part is the plugin's; this owns the reasons, so
//! that one letting go does not turn the screen off under the other.
//!
//! What the reasons want and what the system was last told are two facts: a change is recorded when it is asked for, and
//! `sync` brings the system to it. A system call that fails leaves the system as it was, so that the next `sync` (any
//! reason's, or the page's asking again) tries again. `sync` blocks on the system (Android: the Kotlin side, which answers
//! on the main thread), so it is never run on the main thread: the commands run it from a blocking thread.
use std::sync::{Arc, Mutex};

use sidevoice_mobile_plugin::{keep_screen_on, ScreenError};
use sidevoice_shared::app::DownloadActivity;
use tauri::AppHandle;

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
struct Reasons {
    download: bool,
    call: bool,
}

impl Reasons {
    fn any(self) -> bool {
        self.download || self.call
    }
}

type Apply = Box<dyn Fn(bool) -> Result<(), ScreenError> + Send + Sync>;

/// Keeps the screen on for as long as any reason holds it.
pub struct Screen {
    /// What the reasons want now. Held only to read or change it, never across a system call.
    reasons: Mutex<Reasons>,
    /// Whether the system was last told to keep the screen on. Held across the system call, so that two syncs reach the
    /// system one after the other, each with what the reasons want when it gets there: the last change wins.
    applied: Mutex<bool>,
    apply: Apply,
}

impl Screen {
    pub fn new(app: AppHandle) -> Self {
        Self::with(Box::new(move |on| keep_screen_on(&app, on)))
    }

    fn with(apply: Apply) -> Self {
        Self {
            reasons: Mutex::new(Reasons::default()),
            applied: Mutex::new(false),
            apply,
        }
    }

    /// The call's reason: the person asked for the screen to stay on during the call. Only recorded: `sync` applies it.
    pub fn want_call(&self, on: bool) {
        self.reasons.lock().unwrap().call = on;
    }

    /// The download's reason, recorded the same way.
    fn want_download(&self, on: bool) {
        self.reasons.lock().unwrap().download = on;
    }

    /// Brings the system to what the reasons want, if it is not there. Blocks on the system.
    pub fn sync(&self) -> Result<(), ScreenError> {
        let mut applied = self.applied.lock().unwrap();
        let wanted = self.reasons.lock().unwrap().any();
        if wanted == *applied {
            return Ok(());
        }
        (self.apply)(wanted)?;
        *applied = wanted;
        Ok(())
    }

    /// Records the call's reason and applies it.
    #[cfg(test)]
    fn call(&self, on: bool) -> Result<(), ScreenError> {
        self.want_call(on);
        self.sync()
    }
}

/// The host's download activity, as one reason for the screen.
pub struct DownloadScreen(pub Arc<Screen>);

impl DownloadScreen {
    fn set(&self, on: bool) {
        self.0.want_download(on);
        if let Err(error) = self.0.sync() {
            tracing::warn!(
                keep = on,
                %error,
                "the screen could not be kept on or released"
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
    use std::sync::atomic::{AtomicBool, Ordering};

    fn recorded() -> (Screen, Arc<Mutex<Vec<bool>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let screen = Screen::with(Box::new(move |on| {
            seen.lock().unwrap().push(on);
            Ok(())
        }));
        (screen, calls)
    }

    /// A screen whose system call fails while `failing` is set.
    fn failing() -> (Screen, Arc<AtomicBool>, Arc<Mutex<Vec<bool>>>) {
        let failing = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (broken, seen) = (failing.clone(), calls.clone());
        let screen = Screen::with(Box::new(move |on| {
            seen.lock().unwrap().push(on);
            if broken.load(Ordering::SeqCst) {
                Err(ScreenError("refused".into()))
            } else {
                Ok(())
            }
        }));
        (screen, failing, calls)
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

    #[test]
    fn a_hold_the_system_refused_is_tried_again_by_the_next_ask() {
        let (screen, failing, calls) = failing();
        assert!(screen.call(true).is_err());
        failing.store(false, Ordering::SeqCst);
        screen.call(true).unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            [true, true],
            "the refusal was not taken for a hold"
        );
        screen.call(false).unwrap();
        assert_eq!(*calls.lock().unwrap(), [true, true, false]);
    }

    #[test]
    fn a_release_the_system_refused_leaves_the_screen_held_and_is_tried_again() {
        let (screen, failing, calls) = failing();
        failing.store(false, Ordering::SeqCst);
        screen.call(true).unwrap();
        failing.store(true, Ordering::SeqCst);
        assert!(screen.call(false).is_err());
        failing.store(false, Ordering::SeqCst);
        screen.call(false).unwrap();
        assert_eq!(*calls.lock().unwrap(), [true, false, false]);
        screen.call(false).unwrap();
        assert_eq!(
            calls.lock().unwrap().len(),
            3,
            "once released, asking again is not a system call"
        );
    }

    #[test]
    fn a_refused_hold_does_not_lose_the_reason_of_the_other() {
        let (screen, failing, calls) = failing();
        let downloads = DownloadScreen(Arc::new(screen));
        downloads.started();
        failing.store(false, Ordering::SeqCst);
        downloads.0.call(true).unwrap();
        downloads.ended();
        assert_eq!(
            *calls.lock().unwrap(),
            [true, true],
            "the download's refused hold was made good by the call's"
        );
        downloads.0.call(false).unwrap();
        assert_eq!(*calls.lock().unwrap(), [true, true, false]);
    }

    #[test]
    fn the_last_change_wins_however_the_syncs_are_ordered() {
        let (screen, calls) = recorded();
        screen.want_call(true);
        screen.want_call(false);
        screen.sync().unwrap();
        screen.sync().unwrap();
        assert!(
            calls.lock().unwrap().is_empty(),
            "a hold let go before it was applied never reaches the system"
        );
        screen.want_call(true);
        screen.sync().unwrap();
        assert_eq!(*calls.lock().unwrap(), [true]);
    }
}
