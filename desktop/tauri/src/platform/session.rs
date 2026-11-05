//! What the system says of the person's session that decides whether a held key can still be let go: whether the screen is
//! locked (the keys then go to the lock screen, and a release would never be seen).

#[cfg_attr(target_os = "macos", path = "session/macos.rs")]
#[cfg_attr(target_os = "windows", path = "session/windows.rs")]
#[cfg_attr(target_os = "linux", path = "session/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS.
trait Os {
    /// Whether the screen is locked now. `false` where this system has no observation the app reads.
    fn screen_locked() -> bool {
        false
    }
}

/// Whether the screen is locked now.
pub fn screen_locked() -> bool {
    Native::screen_locked()
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_answers_without_a_session_to_lock() {
        // Only that the observation exists and does not fail here: a CI machine is rarely locked.
        let _ = super::screen_locked();
    }
}
