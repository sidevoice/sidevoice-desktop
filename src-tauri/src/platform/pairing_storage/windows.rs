use std::{fs, path::Path};

pub(super) struct Os;

impl super::Native for Os {
    fn replace_ancestor(ancestor: &Path, moved: &Path, replacement: super::Replacement<'_>) -> bool {
        if let super::Replacement::Redirect(target) = replacement {
            assert!(target.is_dir(), "the redirect target must exist before the held rename attempt");
        }
        assert!(fs::rename(ancestor, moved).is_err(), "held ancestor was renamed during a pairing write");
        false
    }
}
