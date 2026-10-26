//! The native flows on the probe app: the app built with the `probe` feature (CI only, never shipped), started the way
//! a person's machine would, with what the app, the page and the stand-ins log checked. What each OS does differently
//! is in `probe/{macos,windows,linux}.rs`; a flow an OS has no use for says so and does nothing.

use crate::platform::host;
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

    /// A call on the engine's real models through the app's adapter (the ignored tests).
    fn voice_models() -> Result<()> {
        Err(unsupported("voice-models"))
    }

    /// The host's calls on the engine's real models with the process's footprint taken at each step.
    fn voice_memory() -> Result<()> {
        Err(unsupported("voice-memory"))
    }

    /// On the probe app, the same over a long session in the app.
    fn memory() -> Result<()> {
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

/// `probe voice-models`.
pub fn voice_models() -> Result<()> {
    Native::voice_models()
}

/// `probe voice-memory`.
pub fn voice_memory() -> Result<()> {
    Native::voice_memory()
}

/// `probe memory`.
pub fn memory() -> Result<()> {
    Native::memory()
}

/// `probe card`.
pub fn card() -> Result<()> {
    Native::card()
}

/// `probe exit`.
pub fn exit() -> Result<()> {
    Native::exit()
}
