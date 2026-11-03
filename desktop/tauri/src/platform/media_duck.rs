//! What this OS can do to other apps' media while the microphone is open (docs/MEDIA_DUCK.md): observe the players and
//! lower, pause or resume one. What to do, and what to give back, is core's (`media_duck`).
use sidevoice_desktop_core::media_duck::{Lowering, Mode, Pausing, Player, Step};

#[cfg_attr(target_os = "macos", path = "media_duck/macos.rs")]
#[cfg_attr(target_os = "windows", path = "media_duck/windows.rs")]
#[cfg_attr(target_os = "linux", path = "media_duck/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS. Every call may block (COM, D-Bus, a script), so the caller is a worker thread, never the UI's.
trait Os {
    /// How this OS can lower other apps.
    const LOWERING: Lowering = Lowering::None;
    /// How far it can pause them.
    const PAUSING: Pausing = Pausing::None;

    /// The players there are, other than the app's own sound: what plays, and whether each can be lowered or paused. Only
    /// what `mode` needs is asked of the system (a script that asks another app prompts the person for permission).
    fn players(_mode: Mode) -> Vec<Player> {
        Vec::new()
    }

    /// Sets one player's volume, 0 to 1; whether the system took it.
    fn set_volume(_key: &str, _volume: f32) -> bool {
        false
    }

    /// Pauses one player; whether the system took it.
    fn pause(_key: &str) -> bool {
        false
    }

    /// Resumes one player; whether the system took it.
    fn play(_key: &str) -> bool {
        false
    }
}

/// How this OS lowers and pauses, for the page to say so.
pub fn reach() -> (Lowering, Pausing) {
    (Native::LOWERING, Native::PAUSING)
}

/// The players now.
pub fn players(mode: Mode) -> Vec<Player> {
    Native::players(mode)
}

/// Does one step; whether it worked.
pub fn act(step: &Step) -> bool {
    match step {
        Step::SetVolume { key, volume } => Native::set_volume(key, *volume),
        Step::Pause { key } => Native::pause(key),
        Step::Play { key } => Native::play(key),
    }
}
