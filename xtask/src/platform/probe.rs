//! The native flows on the probe app: the app built with the `probe` feature (CI only, never shipped), started the way
//! a person's machine would, with what the app, the page and the stand-ins log checked. What each OS does differently
//! is in `probe/{macos,windows,linux}.rs`; a flow an OS has no use for says so and does nothing.

pub mod checks;
pub mod memory;

use crate::platform::host;
use crate::util::{repo, Cmd};
use crate::Result;

/// A host's way to run each flow. Every method is one real difference between hosts; the default is that the host has
/// no such flow.
trait Os {
    /// The app built with the `probe` feature, over the release app.
    fn probe_app() -> Result<()> {
        Err(unsupported("probe-app"))
    }

    /// On the probe app, the local host from the bundled page: pairing, the proxy, its refusals.
    fn local_host() -> Result<()> {
        Err(unsupported("local-host"))
    }

    /// On the probe app, the probe page: origin, loopback, the native engine, the voice call, the headset.
    fn probe() -> Result<()> {
        Err(unsupported("probe"))
    }

    /// Runs the host's calls on the engine's real models and returns the log with a `memory phase=…` line at each step
    /// (the process's footprint and GPU allocation). The checks on it are common ([`memory`]).
    fn voice_memory_log() -> Result<String> {
        Err(unsupported("voice-memory"))
    }

    /// Runs the probe app through a long session and returns its log with a `memory phase=…` line at each step.
    fn memory_log() -> Result<String> {
        Err(unsupported("memory"))
    }

    /// On the probe app, the call controls card over a full-screen editor.
    fn card() -> Result<()> {
        Err(unsupported("card"))
    }

    /// On the probe app, quitting and restarting with a model on the GPU loaded.
    fn exit() -> Result<()> {
        Err(unsupported("exit"))
    }
}

#[cfg_attr(target_os = "macos", path = "probe/macos.rs")]
#[cfg_attr(target_os = "windows", path = "probe/windows.rs")]
#[cfg_attr(target_os = "linux", path = "probe/linux.rs")]
mod imp;

use imp::Native;

/// The refusal of a flow this host does not have.
fn unsupported(step: &str) -> String {
    format!("`cargo xtask probe {step}` is not available on {}", host::name())
}

/// `probe probe-app`.
pub fn probe_app() -> Result<()> {
    Native::probe_app()
}

/// `probe local-host`.
pub fn local_host() -> Result<()> {
    Native::local_host()
}

/// `probe probe`.
pub fn probe() -> Result<()> {
    Native::probe()
}

/// `probe voice-models`: a call on the engine's real models through the app's adapter: Silero, Whisper tiny on
/// whisper.cpp and Kokoro, downloaded into a scratch folder, or kept in `SIDEVOICE_REAL_CALL_MODELS` (CI caches it);
/// the turn must come back as the clip's words and the reply must be spoken and heard, with injected audio in place of
/// the devices (app-core/src/voice/models/engine/tests.rs). With it, the rest of the voice tests: the native host's
/// lanes over stand-in shared devices, and a call on the machine's own devices, which is ready or fails by a code
/// (app-core/src/voice/host/tests.rs). The same on every OS: nothing in it differs.
pub fn voice_models() -> Result<()> {
    Cmd::new("cargo")
        .args([
            "test",
            "--release",
            "-p",
            "sidevoice-app-core",
            "--lib",
            "voice::",
            "--",
            "--include-ignored",
            "--nocapture",
        ])
        .dir(repo().join("desktop"))
        .run()
}

/// `probe voice-memory`.
pub fn voice_memory() -> Result<()> {
    memory::report(&Native::voice_memory_log()?, memory::VOICE_RULES)
}

/// `probe memory`.
pub fn memory() -> Result<()> {
    memory::report(&Native::memory_log()?, memory::APP_RULES)
}

/// `probe card`.
pub fn card() -> Result<()> {
    Native::card()
}

/// `probe exit`.
pub fn exit() -> Result<()> {
    Native::exit()
}
