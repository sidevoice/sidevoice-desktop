//! The debug files of the app's build, split off its binary between the build and the bundle (docs/CRASH_REPORTS.md → Builds).
//!
//! `profile.release` keeps line tables and does not strip (`debug = "line-tables-only"`, `strip = "none"`): a profile that strips
//! leaves nothing to upload, and on MSVC a build that strips links without a PDB. So `tauri build --no-bundle` leaves a binary
//! with its debug information, and this takes out of it what Sentry needs to name a crash's frames (the frames' addresses and
//! the debug id of the module that holds them are what the app sends) before `tauri bundle` packs the binary. The bundler signs
//! after this, so the binary a signature covers is the one that ships: stripping never invalidates a signature.
//!
//! - Linux: the binary as built, unstripped, is the debug file (line tables, the symbol table and the build id the link adds,
//!   `desktop/.cargo/config.toml`); the binary that ships is `objcopy --strip-debug` (its symbol table stays: function names).
//!   Checked: the debug file has `.debug_line` and a build id, the shipped binary no `.debug_line` and the same id.
//! - macOS: `dsymutil` makes the `.dSYM` from the binary as built, and the binary that ships is `strip -S`. Checked: the `.dSYM`
//!   has DWARF and the UUID of the shipped binary (Sentry matches by it).
//! - Windows: the linker's PDB (the binary only names it by its file name); the NSIS installer never includes it.
//!
//! What is left is in [`dir`], never in a bundle; `cargo xtask sentry upload` uploads it.

use std::path::{Path, PathBuf};

use crate::util::{ensure, fresh_dir, repo};
use crate::Result;

#[cfg_attr(target_os = "macos", path = "debug_files/macos.rs")]
#[cfg_attr(target_os = "windows", path = "debug_files/windows.rs")]
#[cfg_attr(target_os = "linux", path = "debug_files/linux.rs")]
mod imp;
use imp::Native;

/// What differs per OS: how the debug information leaves the binary.
trait Os {
    /// Makes the debug files of `binary` in `out` (empty), and leaves `binary` without debug information of its own, its
    /// symbol table where the OS keeps one in the binary. `build` is the directory cargo made it in (a PDB is beside it).
    fn split(build: &Path, binary: &Path, out: &Path) -> Result<()>;
}

/// The app's executable, without the OS's suffix.
pub const BINARY: &str = "sidevoice-desktop";

/// Where the debug files are left: `target/debug-files`.
pub fn dir() -> PathBuf {
    repo().join("target").join("debug-files")
}

/// Splits the debug files off the binary cargo made in `build` (`target/<triple>/release`, or `target/release`) into [`dir`], and
/// strips the binary of them. Fails if the build left no debug information to take (a profile that strips), so a release never
/// uploads nothing.
pub fn split(build: &Path) -> Result<()> {
    let binary = build.join(sidevoice_platform::target::exe_name(BINARY));
    ensure(binary.is_file(), format!("{} is not there: the build made no binary", binary.display()))?;
    let out = dir();
    fresh_dir(&out)?;
    Native::split(build, &binary, &out)?;
    let made = std::fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))?.count();
    ensure(made > 0, format!("no debug files in {}", out.display()))
}
