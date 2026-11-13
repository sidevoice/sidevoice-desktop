//! What the app's audio session is and does, for the public `audio` log (docs/MOBILE.md → "The audio session log"): the
//! session as the system has it now (category, mode, options, route, output volume, rates and buffers) and what the system
//! says of it (route changes, interruptions, media services lost and reset, the output volume moving).
//!
//! Only iOS keeps such a session for the app (`AVAudioSession`); Android's call service and Telecom route the call, so it
//! has nothing to describe or observe.
use std::sync::Arc;

#[cfg_attr(target_os = "android", path = "audio_session/android.rs")]
#[cfg_attr(target_os = "ios", path = "audio_session/ios.rs")]
mod imp;
use imp::Native;

/// Where what the system says of the session goes, as one line naming the event and its details, from any thread.
pub type Heard = Arc<dyn Fn(String) + Send + Sync>;

/// What differs per OS.
trait Os {
    /// The session as the system has it now, in one line; `None` where there is no session.
    fn describe() -> Option<String> {
        None
    }

    /// Reports to `heard` every change of the route (with its reason and the route before), every interruption (with its
    /// reason), the media services lost and reset, and every move of the output volume, for the rest of the process. An OS
    /// without a session reports nothing.
    fn observe(_heard: Heard) -> Result<(), String> {
        Ok(())
    }
}

/// The session as the system has it now, in one line, or `None` where there is none.
pub fn describe() -> Option<String> {
    Native::describe()
}

/// Starts reporting what the system says of the session to `heard`. Call it once.
pub fn observe(heard: Heard) -> Result<(), String> {
    Native::observe(heard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observing_reports_nothing_by_itself() {
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&heard);
        observe(Arc::new(move |line| sink.lock().unwrap().push(line))).unwrap();
        assert!(heard.lock().unwrap().is_empty());
    }
}
