//! The release steps of the desktop app, one set of commands on every OS (`cargo xtask release <step>`): the app
//! built, checked and started as a release ships it, its installer package made, signed and checksummed. What each OS
//! does differently is in `release/{macos,windows,linux}.rs`; a step an OS has no use for says so and does nothing.

use crate::platform::host;
use crate::util::Channel;
use crate::Result;

/// A host's way to run each release step. Every method is one real difference between hosts; the default is that the
/// host has no such step.
trait Os {
    /// The app bundle, ad-hoc signed; `signed`: Developer ID signed, notarized and stapled by Tauri's bundler. Also
    /// its update artifact for a release or nightly `channel`.
    fn build_app(_signed: bool, _channel: Channel) -> Result<()> {
        Err(unsupported("app"))
    }

    /// What a first open depends on in the built app (`signed`: its signature, notarization and stapling too).
    fn check_app(_signed: bool) -> Result<()> {
        Err(unsupported("check-app"))
    }

    /// The app started with no settings: the bundled interface comes up.
    fn first_open() -> Result<()> {
        Err(unsupported("first-open"))
    }

    /// The installer package (macOS: the `.dmg` with the brand's window), verified, the app inside checked.
    fn package(_signed: bool) -> Result<()> {
        Err(unsupported("package"))
    }

    /// The package signed, notarized, stapled and checked as a download has it.
    fn sign_package() -> Result<()> {
        Err(unsupported("sign-package"))
    }

    /// `dist/SHA256SUMS.txt` for the package.
    fn checksums() -> Result<()> {
        Err(unsupported("checksums"))
    }

    /// Whatever a signing step cut short left (its keychain, certificate and key files).
    fn forget_secrets() -> Result<()> {
        Err(unsupported("forget-secrets"))
    }

    /// The package's window as the system shows it, light and dark (informational).
    fn package_screens() -> Result<()> {
        Err(unsupported("package-screens"))
    }

    /// The app built with the `probe` feature (CI only, never shipped) over the release app.
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

#[cfg_attr(target_os = "macos", path = "release/macos.rs")]
#[cfg_attr(target_os = "windows", path = "release/windows.rs")]
#[cfg_attr(target_os = "linux", path = "release/linux.rs")]
mod imp;

use imp::Native;

/// The refusal of a step this host does not have.
fn unsupported(step: &str) -> String {
    format!("`cargo xtask release {step}` is not available on {}", host::name())
}

/// `release app [--signed] [--channel C]`.
pub fn build_app(signed: bool, channel: Channel) -> Result<()> {
    Native::build_app(signed, channel)
}

/// `release check-app [--signed]`.
pub fn check_app(signed: bool) -> Result<()> {
    Native::check_app(signed)
}

/// `release first-open`.
pub fn first_open() -> Result<()> {
    Native::first_open()
}

/// `release package [--signed]`.
pub fn package(signed: bool) -> Result<()> {
    Native::package(signed)
}

/// `release sign-package`.
pub fn sign_package() -> Result<()> {
    Native::sign_package()
}

/// `release checksums`.
pub fn checksums() -> Result<()> {
    Native::checksums()
}

/// `release forget-secrets`.
pub fn forget_secrets() -> Result<()> {
    Native::forget_secrets()
}

/// `release package-screens`.
pub fn package_screens() -> Result<()> {
    Native::package_screens()
}

/// `release probe-app`.
pub fn probe_app() -> Result<()> {
    Native::probe_app()
}

/// `release local-host`.
pub fn local_host() -> Result<()> {
    Native::local_host()
}

/// `release probe`.
pub fn probe() -> Result<()> {
    Native::probe()
}

/// `release voice-models`.
pub fn voice_models() -> Result<()> {
    Native::voice_models()
}

/// `release voice-memory`.
pub fn voice_memory() -> Result<()> {
    Native::voice_memory()
}

/// `release memory`.
pub fn memory() -> Result<()> {
    Native::memory()
}

/// `release card`.
pub fn card() -> Result<()> {
    Native::card()
}

/// `release exit`.
pub fn exit() -> Result<()> {
    Native::exit()
}
