//! The certificate verifier of the engine's HTTPS (remote providers, model downloads): `reqwest` verifies with the system's
//! own verifier (`rustls-platform-verifier`). On Android that verifier is Kotlin, reached through the JVM, and has to be
//! handed the app's context once, before the first request; the engine cannot do it (it has no JNI environment). iOS and
//! a computer verify through their own libraries and need nothing.
#[cfg_attr(target_os = "android", path = "tls/android.rs")]
#[cfg_attr(target_os = "ios", path = "tls/ios.rs")]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    path = "tls/host.rs"
)]
mod imp;
use imp::Native;

/// What differs per OS: what the system's verifier needs before its first use.
trait Os {
    /// Prepares the verifier. `Err` with why it could not; HTTPS then fails, and nothing else does.
    fn init() -> Result<(), String>;
}

/// Prepares the system's certificate verifier, once, at the app's start: before the engine's first HTTPS request.
pub fn init() -> Result<(), String> {
    Native::init()
}
