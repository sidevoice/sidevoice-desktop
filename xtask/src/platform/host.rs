//! What differs by host in the facts the build tooling reads: its name, the npm launcher and the installers `bundle`
//! makes. Values only, so no per-OS files.

use crate::util::ensure;
use crate::Result;

/// The host's name as `std::env::consts::OS` gives it (`linux`, `macos`, `windows`).
pub fn name() -> &'static str {
    std::env::consts::OS
}

/// Requires `os`: the command's tools are that system's.
pub fn require_os(os: &str, command: &str) -> Result<()> {
    ensure(name() == os, format!("`cargo xtask {command}` runs on {os}"))
}

/// The npm launcher: a `.cmd` on Windows.
pub fn npm() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// The installer formats `tauri build --bundles` makes on this host, if it makes any in `bundle` (macOS has its own
/// commands).
pub fn installer_bundles() -> Option<&'static str> {
    match name() {
        "linux" => Some("deb,appimage"),
        "windows" => Some("nsis"),
        _ => None,
    }
}

/// Whether installers built here must run on other CPUs too, including when bundled on a laptop.
pub fn installers_for_any_cpu() -> bool {
    cfg!(windows)
}
