//! The temporary directory's own paths are short enough, and there is neither a sticky-directory counterpart nor
//! group-only access through this API. Every user may make folders in the system drive's root (`%SystemDrive%\`), and
//! they inherit its "Authenticated Users: Modify".
use super::Area;
use std::path::PathBuf;

pub struct Os;

impl Area for Os {
    fn shared_volume_root() -> Option<PathBuf> {
        let drive = std::env::var_os("SystemDrive")?;
        let mut root = drive.into_string().ok()?;
        root.push('\\');
        Some(PathBuf::from(root))
    }
}
