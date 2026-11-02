//! Windows keeps nothing for the app beyond its folders (the trait's default); a new folder inherits the profile's ACL.

use super::Os;
use std::{fs, io, path::Path};

pub(super) struct Native;

impl Os for Native {
    /// A plain folder: it inherits the user profile's ACL, which already keeps other users out. A private DACL of its
    /// own (`Files::create_private_dir`) would stay on the folder once it is put in place, where the app never had one.
    fn create_folder(path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }
}
