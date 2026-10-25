use super::{Mount, OwnedRemoval};
use std::{fs::File, path::Path};

pub struct NativeOwnedRemoval;

impl OwnedRemoval for NativeOwnedRemoval {
    fn mount(_: &File, _: &Path) -> Result<Mount, &'static str> {
        // HeldDirectory refuses all reparse ancestors, including mounted folders/junctions. Drive/share roots
        // are refused by the common home check. Removal itself also checks descendant volume identity.
        Ok(Mount(Vec::new()))
    }
}
