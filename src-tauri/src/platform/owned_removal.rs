//! Desktop deletion policy over the released held-root removal capability.
//!
//! Writers and reset admission are stopped/held by the transaction. No path comes from the page. The shared
//! library owns recursive preflight, no-follow deletion, entry identity and mount checks within selected trees.
//! Desktop additionally refuses a home at a volume root and mount crossings between home and the held parent.

use sidevoice_platform::fs::{Access, Directory, Files, Fs, HeldDirectory};
use sidevoice_platform::removal::Removal;
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "macos")]
#[path = "owned_removal/macos.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "owned_removal/linux.rs"]
mod imp;
#[cfg(target_os = "windows")]
#[path = "owned_removal/windows.rs"]
mod imp;
pub use imp::NativeOwnedRemoval;

/// Opaque filesystem/mount identity; compared only for equality, never exposed to callers as a native handle.
#[derive(PartialEq)]
pub struct Mount(Vec<u8>);

/// The exact desktop-owned directories and model store for one account home. Selection remains desktop policy.
pub struct Scope {
    directories: Vec<PathBuf>,
    models: PathBuf,
}

/// Owned cleanup and read-only connector preflight. Non-Mac reset remains disabled: those native implementations
/// provide the safety checks but no deletion scope until the separate portable transaction supplies native roots.
pub trait OwnedRemoval {
    /// The existing native desktop allowlist, or no scope when reset cleanup is not available on this host.
    fn scope(_home: &Path) -> Option<Scope> {
        None
    }

    /// Inspect the mount of an already opened directory. This small native hook preserves desktop's existing
    /// home-to-parent guard, which the shared removal API (anchored at its held parent) does not provide.
    fn mount(file: &File, path: &Path) -> Result<Mount, &'static str>;

    /// Preflight an existing tree under home without deleting anything. The selected root must be an ordinary
    /// directory; missing roots succeed. Callers retain their own exact connector-root validation.
    fn check_tree(path: &Path, home: &Path) -> Result<(), &'static str> {
        if let Some(tree) = Prepared::open::<Self>(path, home)? {
            match HeldDirectory::open(path, Access::NotWritableByOthers) {
                Ok(root) => root.check_contents(&[]).map_err(|_| "unsafe-directory")?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(_) => return Err("unsafe-directory"),
            }
            tree.check()?;
        }
        Ok(())
    }

    /// Remove exactly one allowlisted desktop tree. Links are unlinked as entries, never followed; ordinary
    /// hard links are unlinked without altering other names or their contents. All selected descendants are
    /// preflighted before deletion and checked again by the shared capability while deleting.
    fn remove_known(path: &Path, home: &Path) -> Result<(), &'static str> {
        if path == home {
            return Err("home-root");
        }
        let scope = Self::scope(home).ok_or("outside-known-dirs")?;
        if !scope.directories.iter().any(|known| known == path)
            && !Scope::model_children(&scope.models).iter().any(|known| known == path)
        {
            return Err("outside-known-dirs");
        }
        if let Some(tree) = Prepared::open::<Self>(path, home)? {
            tree.remove()?;
        }
        Ok(())
    }

    /// Preflight all three engine-owned children before deleting any, preserving the model root and every
    /// unlisted sibling. An absent store stays absent. Redirected model roots are refused, not traversed.
    fn remove_model_store(root: &Path, home: &Path) -> Result<(), &'static str> {
        let scope = Self::scope(home).ok_or("outside-known-dirs")?;
        if root != scope.models {
            return Err("outside-known-dirs");
        }
        let mut trees = Vec::new();
        for child in Scope::model_children(root) {
            if let Some(tree) = Prepared::open::<Self>(&child, home)? {
                tree.check()?;
                trees.push(tree);
            }
        }
        for tree in trees {
            tree.remove()?;
        }
        Ok(())
    }
}

impl Scope {
    fn model_children(root: &Path) -> [PathBuf; 3] {
        ["blobs", "models", "partial"].map(|name| root.join(name))
    }
}

/// All ancestors from the home remain held until deletion, and are verified again before each operation. The
/// underlying capability also holds the full root-to-parent chain and rechecks it during the recursive removal.
struct Prepared {
    ancestors: Vec<HeldDirectory>,
    name: OsString,
}

impl Prepared {
    fn clean_absolute(path: &Path) -> bool {
        path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir | Component::Normal(_)))
    }

    fn open<R: OwnedRemoval + ?Sized>(path: &Path, home: &Path) -> Result<Option<Self>, &'static str> {
        if path == home {
            return Err("home-root");
        }
        if !Self::clean_absolute(home)
            || !Self::clean_absolute(path)
            || home.components().filter(|part| matches!(part, Component::Normal(_))).count() < 2
        {
            return Err("unsafe-path");
        }
        let relative = path.strip_prefix(home).map_err(|_| "outside-home")?;
        if fs::canonicalize(home).ok().as_deref() != Some(home) {
            return Err("unsafe-home");
        }
        let held_home = HeldDirectory::open(home, Access::NotWritableByOthers).map_err(|_| "unsafe-home")?;
        let home_file = Files::open_read(home).map_err(|_| "unsafe-home")?;
        let expected = R::mount(&home_file, home)?;
        let outer = home.parent().ok_or("unsafe-home")?;
        let outer_file = Files::open_read(outer).map_err(|_| "inspect-mount")?;
        if R::mount(&outer_file, outer)? != expected {
            return Err("volume-root");
        }
        held_home.check_contents(&[]).map_err(|_| "changed-path")?;
        let mut ancestors = vec![held_home];
        let mut prefix = home.to_path_buf();
        let mut parts = relative.components().peekable();
        while let Some(part) = parts.next() {
            let Component::Normal(name) = part else { return Err("unsafe-path") };
            if parts.peek().is_none() {
                let prepared = Self { ancestors, name: name.to_owned() };
                prepared.verify()?;
                return Ok(Some(prepared));
            }
            prefix.push(name);
            let held = match HeldDirectory::open(&prefix, Access::NotWritableByOthers) {
                Ok(held) => held,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err("unsafe-directory"),
            };
            let file = Files::open_read(&prefix).map_err(|_| "unsafe-directory")?;
            if R::mount(&file, &prefix)? != expected {
                return Err("mount-boundary");
            }
            held.check_contents(&[]).map_err(|_| "changed-path")?;
            ancestors.push(held);
        }
        Err("unsafe-path")
    }

    fn verify(&self) -> Result<(), &'static str> {
        for held in &self.ancestors {
            held.check_contents(&[]).map_err(|_| "changed-path")?;
        }
        Ok(())
    }

    fn check(&self) -> Result<(), &'static str> {
        self.verify()?;
        self.ancestors.last().ok_or("unsafe-path")?.check_contents(&[&self.name]).map_err(|_| "unsafe-entry")
    }

    fn remove(&self) -> Result<(), &'static str> {
        self.verify()?;
        self.ancestors.last().ok_or("unsafe-path")?.remove_contents(&[&self.name]).map_err(|_| "remove")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_platform::fs::TestFiles;

    struct Fixture;

    impl OwnedRemoval for Fixture {
        fn scope(home: &Path) -> Option<Scope> {
            Some(Scope {
                directories: vec![home.join("state"), home.join("mounted/state")],
                models: home.join("store"),
            })
        }

        fn mount(file: &File, path: &Path) -> Result<Mount, &'static str> {
            NativeOwnedRemoval::mount(file, path)
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::Builder::new().tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        Files::shut_others_out(temp.path());
        let home = temp.path().join("home");
        Files::create_private_dir(&home).unwrap();
        (temp, home)
    }

    fn directory(path: &Path) {
        Files::create_private_dir(path).unwrap();
    }

    fn put(path: &Path) {
        Files::create_private(path, b"preserve outside owned names").unwrap();
    }

    #[test]
    fn only_exact_owned_names_are_removed_and_missing_roots_stay_missing() {
        let (_temp, home) = fixture();
        let selected = home.join("state");
        assert_eq!(Fixture::remove_known(&selected, &home), Ok(()));
        assert!(!selected.exists());
        assert_eq!(Fixture::remove_model_store(&home.join("store"), &home), Ok(()));
        assert!(!home.join("store").exists());
        directory(&selected);
        put(&selected.join("payload"));
        put(&home.join("unrelated"));
        for refused in [&home, &home.join("unrelated"), &home.join("state/../unrelated"), Path::new("relative")] {
            assert!(Fixture::remove_known(refused, &home).is_err());
        }
        assert_eq!(Fixture::remove_known(&selected, &home), Ok(()));
        assert!(!selected.exists());
        assert!(home.join("unrelated").is_file());
        assert!(Fixture::remove_model_store(&home, &home).is_err());
    }

    #[test]
    fn links_and_shortcut_files_do_not_stop_cleanup_or_touch_outside_targets() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        directory(&outside);
        put(&outside.join("sentinel"));
        let tree = home.join("state");
        directory(&tree);
        Files::symlink(&outside, tree.join("directory-link"));
        Files::symlink(outside.join("sentinel"), tree.join("file-link"));
        Files::symlink(outside.join("missing"), tree.join("dangling"));
        put(&tree.join("ordinary"));
        // A shell shortcut is an ordinary file, irrespective of the platform running this fixture.
        put(&tree.join("shortcut.lnk"));
        assert_eq!(Fixture::remove_known(&tree, &home), Ok(()));
        assert!(!tree.exists());
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve outside owned names");
        Files::symlink(&outside, &tree);
        assert_eq!(Fixture::remove_known(&tree, &home), Ok(()));
        assert!(fs::symlink_metadata(&tree).is_err());
        assert!(outside.join("sentinel").is_file());
    }

    #[test]
    fn hard_link_cleanup_only_unlinks_the_owned_entry() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        put(&outside);
        let tree = home.join("state");
        directory(&tree);
        fs::hard_link(&outside, tree.join("linked")).unwrap();
        put(&tree.join("ordinary"));
        assert_eq!(Fixture::remove_known(&tree, &home), Ok(()));
        assert!(!tree.exists());
        assert_eq!(fs::read(&outside).unwrap(), b"preserve outside owned names");
    }

    #[test]
    fn models_keep_unlisted_siblings_and_unlink_selected_links() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        directory(&outside);
        put(&outside.join("sentinel"));
        let root = home.join("store");
        directory(&root);
        Files::symlink(&outside, root.join("blobs"));
        for name in ["models", "partial", "other-program"] {
            directory(&root.join(name));
            put(&root.join(name).join("payload"));
        }
        assert_eq!(Fixture::remove_model_store(&root, &home), Ok(()));
        for child in Scope::model_children(&root) {
            assert!(fs::symlink_metadata(child).is_err());
        }
        assert!(root.join("other-program/payload").is_file());
        assert!(outside.join("sentinel").is_file());
    }

    #[test]
    fn every_model_child_is_preflighted_before_any_is_deleted() {
        let (_temp, home) = fixture();
        let root = home.join("store");
        directory(&root);
        for child in Scope::model_children(&root) {
            directory(&child);
            put(&child.join("payload"));
        }
        Files::let_others_change(&root.join("partial"));
        assert!(Fixture::remove_model_store(&root, &home).is_err());
        for child in Scope::model_children(&root) {
            assert!(child.join("payload").is_file());
        }
        Files::shut_others_out(&root.join("partial"));
    }

    #[test]
    fn redirected_home_and_model_parent_are_refused() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        directory(&outside);
        directory(&outside.join("blobs"));
        put(&outside.join("blobs/sentinel"));
        Files::symlink(&outside, home.join("store"));
        assert!(Fixture::remove_model_store(&home.join("store"), &home).is_err());
        let alias = temp.path().join("home-alias");
        Files::symlink(&home, &alias);
        assert!(Fixture::remove_known(&alias.join("state"), &alias).is_err());
        assert!(outside.join("blobs/sentinel").is_file());
    }

    #[test]
    fn replacement_of_a_held_parent_is_refused_or_prevented() {
        let (temp, home) = fixture();
        directory(&home.join("state"));
        put(&home.join("state/payload"));
        let tree = Prepared::open::<Fixture>(&home.join("state"), &home).unwrap().unwrap();
        tree.check().unwrap();
        let moved = temp.path().join("moved");
        match fs::rename(&home, &moved) {
            Ok(()) => {
                directory(&home);
                directory(&home.join("state"));
                put(&home.join("state/decoy"));
                assert!(tree.remove().is_err());
                assert!(moved.join("state/payload").is_file());
                assert!(home.join("state/decoy").is_file());
            }
            Err(_) => {
                // A held Windows ancestor excludes delete sharing, preventing replacement outright.
                assert!(home.join("state/payload").is_file());
                tree.remove().unwrap();
                assert!(!home.join("state").exists());
            }
        }
    }

    #[test]
    fn read_only_preflight_preserves_entries_and_refuses_a_redirected_root() {
        let (temp, home) = fixture();
        let tree = home.join("state");
        directory(&tree);
        put(&tree.join("payload"));
        Files::symlink(temp.path().join("absent"), tree.join("link"));
        assert_eq!(Fixture::check_tree(&tree, &home), Ok(()));
        assert!(tree.join("payload").is_file());
        Files::symlink(&tree, home.join("alias"));
        assert!(Fixture::check_tree(&home.join("alias"), &home).is_err());
        assert!(Fixture::check_tree(&home, &home).is_err());
        assert!(Fixture::check_tree(temp.path(), &home).is_err());
    }

    struct ChangedMount;

    impl OwnedRemoval for ChangedMount {
        fn scope(home: &Path) -> Option<Scope> {
            Fixture::scope(home)
        }

        fn mount(file: &File, path: &Path) -> Result<Mount, &'static str> {
            if path.file_name().is_some_and(|name| name == "mounted") {
                Ok(Mount(b"foreign-mount".to_vec()))
            } else {
                NativeOwnedRemoval::mount(file, path)
            }
        }
    }

    #[test]
    fn mount_root_and_intermediate_mount_refusals_precede_deletion() {
        let (_temp, home) = fixture();
        let mounted = home.join("mounted");
        directory(&mounted);
        directory(&mounted.join("state"));
        put(&mounted.join("state/sentinel"));
        assert_eq!(ChangedMount::remove_known(&mounted.join("state"), &home), Err("mount-boundary"));
        assert_eq!(ChangedMount::remove_known(&mounted.join("state"), &mounted), Err("volume-root"));
        assert!(mounted.join("state/sentinel").is_file());
    }

    #[test]
    fn native_scope_has_only_the_existing_desktop_names_or_refuses_cleanup() {
        let (_temp, home) = fixture();
        if let Some(scope) = NativeOwnedRemoval::scope(&home) {
            assert_eq!(scope.directories.len(), 5);
            assert_eq!(scope.models, home.join("Library/Application Support/Sidevoice/Models"));
            for path in &scope.directories {
                assert_eq!(NativeOwnedRemoval::remove_known(path, &home), Ok(()));
            }
        } else {
            assert_eq!(NativeOwnedRemoval::remove_known(&home.join("state"), &home), Err("outside-known-dirs"));
        }
    }
}
