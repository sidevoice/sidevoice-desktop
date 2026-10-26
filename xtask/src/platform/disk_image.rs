//! Building the installer disk image of an app bundle.

use std::path::Path;

use crate::Result;

/// A host's way to assemble a `.dmg` with the brand's window.
pub trait DiskImage {
    /// Writes `output` from the `.app` bundle `app`: `background` is the window drawing at 1x and at 2x, `layout`
    /// the icon layout (`layout.json`) and `icon` the volume icon. `work` is a scratch directory the build may fill.
    /// A host that cannot build a `.dmg` returns an error saying so and writes nothing.
    fn assemble(
        &self,
        work: &Path,
        app: &Path,
        background: [&Path; 2],
        layout: &Path,
        icon: &Path,
        output: &Path,
    ) -> Result<()>;
}

#[cfg(target_os = "macos")]
#[path = "disk_image/macos.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
#[path = "disk_image/other.rs"]
mod imp;

pub use imp::Host;
