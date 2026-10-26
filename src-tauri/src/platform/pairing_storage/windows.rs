use std::{fs, path::Path};

pub(super) struct Native;

impl super::Os for Native {
    fn replace_ancestor(ancestor: &Path, moved: &Path, replacement: super::Replacement<'_>) -> bool {
        if let super::Replacement::Redirect(target) = replacement {
            assert!(target.is_dir(), "the redirect target must exist before the held rename attempt");
        }
        assert!(fs::rename(ancestor, moved).is_err(), "held ancestor was renamed during a pairing write");
        false
    }
}
