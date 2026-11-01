//! The system taking the call's audio away and giving it back (D-AUDIO-FOCUS): a phone call, an alarm, another app that
//! plays. Only iOS tells the app itself, as `AVAudioSession`'s interruption notifications, which the process observes for
//! the session the call activates. Android tells the call's service, which holds the audio focus
//! (`sidevoice-mobile-plugin`'s `CallEvent::Focus`), so there is nothing to observe here.
//!
//! This reports what the system said. Whether it pauses the call, and how it resumes it, is the call's decision
//! (`call.rs`).
use std::sync::Arc;

#[cfg_attr(target_os = "android", path = "interruption/android.rs")]
#[cfg_attr(target_os = "ios", path = "interruption/ios.rs")]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    path = "interruption/host.rs"
)]
mod imp;
use imp::Native;

/// What the system said of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interruption {
    /// The system took the audio away: the session is deactivated, and nothing plays or records until it ends.
    Began,
    /// The interruption is over. `resume` is the system's own advice on whether to go on (`shouldResume`): it is not given
    /// when the interruption was another app that goes on playing.
    Ended { resume: bool },
}

/// Where the interruptions go, from any thread.
pub type Listener = Arc<dyn Fn(Interruption) + Send + Sync>;

/// What differs per OS: whether the system tells the app about interruptions.
trait Os {
    /// Reports every interruption of the app's audio session to `listener`, for the rest of the process. An OS that does
    /// not tell the app (Android: the call's service does) reports none.
    fn observe(_listener: Listener) -> Result<(), String> {
        Ok(())
    }
}

/// Starts reporting the system's interruptions of the app's audio session to `listener`. Call it once.
///
/// # Errors
///
/// `audio-session-interruptions-unavailable` when the system does not offer them.
pub fn observe(listener: Listener) -> Result<(), String> {
    Native::observe(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observing_succeeds_and_reports_nothing_by_itself() {
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&heard);
        observe(Arc::new(move |event| sink.lock().unwrap().push(event))).unwrap();
        assert!(heard.lock().unwrap().is_empty());
    }
}
