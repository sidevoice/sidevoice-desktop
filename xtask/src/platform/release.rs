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
