//! What this OS can do to other apps' media while the microphone is open (docs/MEDIA_DUCK.md): observe the players and
//! lower, pause or resume one. What to do, and what to give back, is core's (`media_duck`).
use sidevoice_desktop_core::media_duck::{Access, Lowering, Mode, Pausing, Player, Seen, Step};

#[cfg_attr(target_os = "macos", path = "media_duck/macos.rs")]
#[cfg_attr(target_os = "windows", path = "media_duck/windows.rs")]
#[cfg_attr(target_os = "linux", path = "media_duck/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS. Every call may block (COM, D-Bus, a script), so the caller is a worker thread, never the UI's.
trait Os {
    /// How this OS can lower other apps, as far as it can be told (a tool it needs may be missing).
    fn lowering() -> Lowering {
        Lowering::None
    }

    /// How far it can pause them.
    fn pausing() -> Pausing {
        Pausing::None
    }

    /// Whether the system lets the app control the apps it pauses. Never asks the person.
    fn access() -> Access {
        Access::Granted
    }

    /// Asks the person for that access, if the system does that; the system may show a dialog.
    fn ask() {}

    /// The players there are, other than this app's own sound and everything it started: what each does, and whether it can
    /// be lowered or paused. Only what `mode` needs is asked of the system.
    fn players(_mode: Mode) -> Vec<Player> {
        Vec::new()
    }

    /// One player again, by its key (on the device it was taken from, not the default's) or, failing that, the next player of
    /// its app, where the system remembers a volume per app.
    fn player(_key: &str, _app: &str) -> Seen {
        Seen::Gone
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
    (Native::lowering(), Native::pausing())
}

/// Whether the system lets the app pause what it pauses.
pub fn access() -> Access {
    Native::access()
}

/// Asks for that access (the person picked Pause).
pub fn ask() {
    Native::ask()
}

/// The players now.
pub fn players(mode: Mode) -> Vec<Player> {
    Native::players(mode)
}

/// One player again.
pub fn player(key: &str, app: &str) -> Seen {
    Native::player(key, app)
}

/// When this machine started, in seconds since the epoch: what tells a debt of an earlier run from this one's.
pub fn boot_time() -> u64 {
    sysinfo::System::boot_time()
}

/// Does one step; whether it worked.
pub fn act(step: &Step) -> bool {
    match step {
        Step::SetVolume { key, volume } => Native::set_volume(key, *volume),
        Step::Pause { key } => Native::pause(key),
        Step::Play { key } => Native::play(key),
    }
}
