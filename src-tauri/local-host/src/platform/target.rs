//! Select the npm package for this computer's operating system and architecture.

/// Resolve the host's package name, or decline an unsupported target before any download.
/// The only host-dependent inputs are the OS and architecture values; the release table is pure.
pub fn current_package() -> Option<String> {
    crate::npm::platform_package(std::env::consts::OS, std::env::consts::ARCH)
}
