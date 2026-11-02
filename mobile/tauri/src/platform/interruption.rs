//! The system taking the call's audio away and giving it back (D-AUDIO-FOCUS): a phone call, an alarm, another app that
//! plays. Only iOS tells the app itself, as `AVAudioSession`'s interruption notifications, which the process observes for
//! the session the call activates. Android tells the call's service, which holds the audio focus
//! (`sidevoice-mobile-plugin`'s `CallEvent::Focus`), so there is nothing to observe here.
//!
//! This reports what the system said. Whether it pauses the call, and how it resumes it, is the call's decision
//! (`call.rs`).
//!
//! iOS also says when its media services were reset (`AVAudioSessionMediaServicesWereResetNotification`): the session's
//! declaration is gone with them, and the app declares it again.
use std::sync::Arc;

#[cfg_attr(target_os = "android", path = "interruption/android.rs")]
#[cfg_attr(target_os = "ios", path = "interruption/ios.rs")]
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
    /// The system's media services were reset: the session's declaration (its category, mode and options) is gone, and
    /// the audio of the process has to be declared and set up again.
    Reset,
}

/// Where the interruptions go, from any thread.
pub type Listener = Arc<dyn Fn(Interruption) + Send + Sync>;

/// What an OS observes of one: whether it `began` (else it ended) and, for an end, whether the system advises resuming.
type Observed = Arc<dyn Fn(bool, bool) + Send + Sync>;

/// What an OS observes of a reset of its media services.
type ObservedReset = Arc<dyn Fn() + Send + Sync>;

fn interruption(began: bool, should_resume: bool) -> Interruption {
    if began {
        Interruption::Began
    } else {
        Interruption::Ended {
            resume: should_resume,
        }
    }
}

/// What differs per OS: whether the system tells the app about interruptions, and about resets of its media services.
trait Os {
    /// Reports every interruption of the app's audio session to `listener`, for the rest of the process. An OS that does
    /// not tell the app (Android: the call's service does) reports none.
    fn observe(_observed: Observed) -> Result<(), String> {
        Ok(())
    }

    /// Reports every reset of the system's media services to `reset`, for the rest of the process. An OS without them
    /// reports none.
    fn observe_reset(_reset: ObservedReset) -> Result<(), String> {
        Ok(())
    }
}

/// Starts reporting the system's interruptions of the app's audio session, and the resets of its media services, to
/// `listener`. Call it once.
///
/// # Errors
///
/// `audio-session-interruptions-unavailable` when the system does not offer them.
pub fn observe(listener: Listener) -> Result<(), String> {
    let reset = Arc::clone(&listener);
    Native::observe_reset(Arc::new(move || reset(Interruption::Reset)))?;
    Native::observe(Arc::new(move |began, should_resume| {
        listener(interruption(began, should_resume));
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_os_observed_is_what_the_call_is_told() {
        assert_eq!(interruption(true, false), Interruption::Began);
        assert_eq!(interruption(true, true), Interruption::Began);
        assert_eq!(
            interruption(false, true),
            Interruption::Ended { resume: true }
        );
        assert_eq!(
            interruption(false, false),
            Interruption::Ended { resume: false }
        );
    }

    #[test]
    fn observing_succeeds_and_reports_nothing_by_itself() {
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&heard);
        observe(Arc::new(move |event| sink.lock().unwrap().push(event))).unwrap();
        assert!(heard.lock().unwrap().is_empty());
    }
}
