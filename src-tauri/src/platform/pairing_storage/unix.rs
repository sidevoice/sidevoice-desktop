use std::{fs, path::Path};

use sidevoice_platform::fs::{Files, TestFiles};

pub struct Os;

impl super::PairingRace for Os {
    fn replace_ancestor(ancestor: &Path, moved: &Path, replacement: super::Replacement<'_>) -> bool {
        fs::rename(ancestor, moved).unwrap();
        match replacement {
            super::Replacement::Redirect(target) => Files::symlink(target, ancestor),
            super::Replacement::Directory => fs::create_dir(ancestor).unwrap(),
        }
        true
    }
}
