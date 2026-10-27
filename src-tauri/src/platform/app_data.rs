//! Deleting the app's data on this computer. The reset transaction in `data.rs` keeps the sequencing, the journal
//! and report, and the relaunch; it runs [`delete_data`] in the reset helper after the app has exited, and asks only
//! for each step's result. Removing the app itself is `uninstall`.
//!
//! [`delete_data`] deletes, on every OS, the folders the app already knows, then whatever more the OS keeps for it
//! (the only per-OS part). The folders are the ones Tauri gives the app, resolved by the rule Tauri documents for its
//! `PathResolver` (the helper runs before Tauri, so it has no `AppHandle`): `dirs` places plus the bundle identifier,
//! the logs at `<local data>/<id>/logs` (macOS keeps them in `~/Library/Logs/<id>`, one of its extras). With them,
//! the shared model store (`sidevoice-platform`'s `data_dir`) and the browser storage `webview_data` names. A folder
//! inside another one is removed with it. Nothing comes from the page or from the reset plan.
//!
//! `home` must still be the account home (`account::home`) when the deletion starts; another home (an alternate
//! profile supplied as `HOME`) is refused before anything is deleted.
//!
//! The account home is checked once (its own canonical path, not a volume's root) and held. Every directory a
//! deletion works in is then reached beneath that held home with `Directory::open_beneath`, one component at a time
//! relative to the handle before it: no step goes by path between the check and the handle the removal uses, and a
//! link, junction or other reparse point on the way (the directory itself included) or a change of device, mount or
//! volume is refused. Every removal goes through the shared held-root capability (`HeldDirectory` + `Removal`): the
//! selected entry and its whole tree are preflighted before anything is deleted, links are unlinked and never
//! followed, entry identity and mounts below the held parent are checked while deleting.

use crate::platform::account;
use crate::platform::webview_data::{self, Released};
use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
use sidevoice_platform::removal::Removal;
use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

#[cfg_attr(target_os = "macos", path = "app_data/macos.rs")]
#[cfg_attr(target_os = "windows", path = "app_data/windows.rs")]
#[cfg_attr(target_os = "linux", path = "app_data/linux.rs")]
mod imp;
use imp::Native;

/// The app's bundle identifier (`tauri.conf.json`), which names its folders on every host.
const IDENTIFIER: &str = "dev.sidevoice.desktop";

/// The engine-owned children of the model store; the store itself and any other sibling stay.
const MODEL_CHILDREN: [&str; 3] = ["blobs", "models", "partial"];

/// Why a reset operation (this deletion, or `uninstall`) did not complete.
pub enum Error {
    /// This host cannot do it.
    Unsupported,
    /// The step sink asked to stop (its journal could not be written); later steps did not run.
    Stopped,
    /// It failed, with the reset report's key.
    Failed(&'static str),
}

/// Told each step's name and result, in order, as the report names them (`app-data`, `models`, `webview`, `cache`,
/// `logs`, then the OS's own: on macOS `logs`, `saved-state` and `preferences`); a failed step is the report's key.
/// Returning `false` stops the deletion.
pub type Steps<'a> = &'a mut dyn FnMut(&'static str, Result<(), &'static str>) -> bool;

/// Delete all the app's data for the account home `home` (`account::home`), in order, reporting each step to
/// `steps`: the app's folders, then what the OS keeps for it. Browser storage is removed only when `browser` proves
/// its writers released (`webview_data`); otherwise those steps fail with `reset.process-busy` and stay. A missing
/// folder is a completed step.
pub fn delete_data(home: &Path, browser: &io::Result<Released>, steps: Steps) -> Result<(), Error> {
    let home = Home::open(home);
    delete_app_folders(&home, browser, steps)?;
    Native::delete_extra_data(&home, steps)
}

/// Read-only: whether an existing protected directory under `home` (the connector's data and releases) and its
/// whole tree could be removed. It must be an ordinary directory, reached from `home` with no link or volume
/// change. A missing directory passes. The `Err` is a short reason.
pub fn check_tree(home: &Path, path: &Path) -> Result<(), &'static str> {
    let (parent, name) = split(path)?;
    match Home::open(home).reach(parent)? {
        Some(held) => check_entry(&held, name),
        None => Ok(()),
    }
}

/// What the OS keeps for the app beyond its folders. Hosts with nothing more keep the default.
trait Os {
    /// Delete it inside the account home `home`, reporting each step to `steps`.
    fn delete_extra_data(_home: &Home, _steps: Steps) -> Result<(), Error> {
        Ok(())
    }
}

/// The app folders every OS has, once `home` is checked to still be the account home.
fn delete_app_folders(home: &Home, browser: &io::Result<Released>, steps: Steps) -> Result<(), Error> {
    let places = match account::home() {
        Ok(account) if account == home.path => Places::native(account),
        _ => Err("unsafe-home"),
    };
    delete_folders(home, places, browser, steps)
}

/// Delete the folders of `places`, or report why there are none before deleting anything.
fn delete_folders(
    home: &Home,
    places: Result<Places, &'static str>,
    browser: &io::Result<Released>,
    steps: Steps,
) -> Result<(), Error> {
    match places {
        Ok(places) => Folders::from(places).delete(home, browser, steps),
        Err(key) => {
            steps("app-data", Err(key));
            Err(Error::Failed(key))
        }
    }
}

/// `path`'s parent and its own name.
fn split(path: &Path) -> Result<(&Path, &OsStr), &'static str> {
    Ok((path.parent().ok_or("unsafe-path")?, path.file_name().ok_or("unsafe-path")?))
}

/// Read-only: whether the entry `name` in `parent` is an existing ordinary directory (no link) whose whole tree could
/// be removed. A missing entry passes.
fn check_entry(parent: &HeldDirectory, name: &OsStr) -> Result<(), &'static str> {
    match parent.open_beneath(Path::new(name), Access::NotWritableByOthers) {
        Ok(_) => parent.check_contents(&[name]).map_err(|_| "unsafe-entry"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("unsafe-directory"),
    }
}

/// The places Tauri, the model store and the browser storage resolve to for this account.
struct Places {
    data: PathBuf,
    cache: PathBuf,
    logs: PathBuf,
    models: PathBuf,
    webview: Vec<PathBuf>,
}

/// One folder to delete, under the name the report gives it.
struct Folder {
    name: &'static str,
    path: PathBuf,
}

/// The app's folders, in deletion order, with what tells browser storage apart.
struct Folders {
    models: PathBuf,
    webview: Vec<PathBuf>,
    list: Vec<Folder>,
}

impl Places {
    /// For the account `home` (`account::home`).
    fn native(home: PathBuf) -> Result<Self, &'static str> {
        let unavailable = "reset.unavailable";
        let webview = webview_data::locations(&home).map_err(|_| unavailable)?.paths().to_vec();
        Ok(Self {
            data: dirs::data_dir().ok_or(unavailable)?.join(IDENTIFIER),
            cache: dirs::cache_dir().ok_or(unavailable)?.join(IDENTIFIER),
            logs: dirs::data_local_dir().ok_or(unavailable)?.join(IDENTIFIER).join("logs"),
            models: sidevoice_platform::data_dir::models_dir().ok_or(unavailable)?,
            webview,
        })
    }
}

impl Folders {
    /// `app-data`, `models`, browser storage that is no other folder (`webview`), `cache`, `logs`; a folder equal
    /// to or inside an earlier one is dropped (deleting the earlier one removes it).
    fn from(places: Places) -> Self {
        let mut candidates = vec![("app-data", places.data.clone()), ("models", places.models.clone())];
        for path in &places.webview {
            if *path != places.data && *path != places.cache {
                candidates.push(("webview", path.clone()));
            }
        }
        candidates.push(("cache", places.cache));
        candidates.push(("logs", places.logs));
        let mut list: Vec<Folder> = Vec::new();
        for (name, path) in candidates {
            if !list.iter().any(|kept| path.starts_with(&kept.path)) {
                list.push(Folder { name, path });
            }
        }
        Self { models: places.models, webview: places.webview, list }
    }

    /// Each folder in turn, reached beneath the held `home`.
    fn delete(&self, home: &Home, browser: &io::Result<Released>, steps: Steps) -> Result<(), Error> {
        for folder in &self.list {
            let result = if self.webview.contains(&folder.path) {
                match browser {
                    Ok(released) => home.remove_browser_storage(released, &folder.path),
                    Err(_) => Err("reset.process-busy"),
                }
            } else if folder.path == self.models {
                self.remove_models(home)
            } else {
                home.remove_entry(&folder.path)
            };
            if !steps(folder.name, result) {
                return Err(Error::Stopped);
            }
        }
        Ok(())
    }

    /// The engine-owned children only: the shared capability preflights all of them before removing any. The
    /// store itself must be an ordinary directory; a child may be a link, which is unlinked.
    fn remove_models(&self, home: &Home) -> Result<(), &'static str> {
        match home.reach(&self.models)? {
            Some(held) => held.remove_contents(&MODEL_CHILDREN.map(OsStr::new)).map_err(|_| "remove"),
            None => Ok(()),
        }
    }
}

/// The account home, checked once and held: every directory a deletion works in is reached beneath it, never by path.
/// `held` is why it could not be, reported by each step that needs it.
struct Home {
    path: PathBuf,
    held: Result<HeldDirectory, &'static str>,
}

/// A directory reached beneath the held home: the home itself, or one opened beneath it.
enum Held<'a> {
    Home(&'a HeldDirectory),
    Beneath(HeldDirectory),
}

impl std::ops::Deref for Held<'_> {
    type Target = HeldDirectory;

    fn deref(&self) -> &HeldDirectory {
        match self {
            Held::Home(home) => home,
            Held::Beneath(held) => held,
        }
    }
}

impl Home {
    /// Check that `path` is a clean absolute path, its own canonical form, and not a volume's root (its parent is
    /// on its volume), then hold it. The account home's parent is not this user's to replace, so the home held is
    /// the one checked.
    fn open(path: &Path) -> Self {
        Self { path: path.to_path_buf(), held: Self::check(path) }
    }

    fn check(path: &Path) -> Result<HeldDirectory, &'static str> {
        if !clean_absolute(path) || path.components().filter(|part| matches!(part, Component::Normal(_))).count() < 2 {
            return Err("unsafe-path");
        }
        if dunce::canonicalize(path).ok().as_deref() != Some(path) {
            return Err("unsafe-home");
        }
        let expected = volume(path).map_err(|_| "unsafe-home")?;
        let outer = path.parent().ok_or("unsafe-home")?;
        if volume(outer).map_err(|_| "inspect-mount")? != expected {
            return Err("volume-root");
        }
        HeldDirectory::open(path, Access::NotWritableByOthers).map_err(|_| "unsafe-home")
    }

    /// Hold `dir` (inside or equal to the home), opened beneath the held home without following any link and on its
    /// device, mount and volume. A missing directory on the way is `None`.
    fn reach(&self, dir: &Path) -> Result<Option<Held<'_>>, &'static str> {
        let home = self.held.as_ref().map_err(|key| *key)?;
        if !clean_absolute(dir) {
            return Err("unsafe-path");
        }
        let relative = dir.strip_prefix(&self.path).map_err(|_| "outside-home")?;
        if relative.as_os_str().is_empty() {
            return Ok(Some(Held::Home(home)));
        }
        match home.open_beneath(relative, Access::NotWritableByOthers) {
            Ok(held) => Ok(Some(Held::Beneath(held))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("unsafe-directory"),
        }
    }

    /// Remove one entry the app owns (not browser storage, not the model store). The entry itself may be a link,
    /// which is unlinked.
    fn remove_entry(&self, path: &Path) -> Result<(), &'static str> {
        let (parent, name) = split(path)?;
        match self.reach(parent)? {
            Some(held) => held.remove_contents(&[name]).map_err(|_| "remove"),
            None => Ok(()),
        }
    }

    /// Remove one browser storage root, released (`webview_data`). It must be an ordinary directory, never a link.
    fn remove_browser_storage(&self, released: &Released, path: &Path) -> Result<(), &'static str> {
        let (parent, name) = split(path)?;
        match self.reach(parent)? {
            Some(held) => {
                check_entry(&held, name).and_then(|_| webview_data::remove(released, &held, path).map_err(|_| "remove"))
            }
            None => Ok(()),
        }
    }
}

fn clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir | Component::Normal(_)))
}

/// The volume a path is on (links followed: only used on the canonical home and its parent).
fn volume(path: &Path) -> io::Result<u64> {
    Ok(match file_id::get_file_id(path)? {
        file_id::FileId::Inode { device_id, .. } => device_id,
        file_id::FileId::LowRes { volume_serial_number, .. } => volume_serial_number.into(),
        file_id::FileId::HighRes { volume_serial_number, .. } => volume_serial_number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_platform::fs::{Files, Fs, TestFiles};
    use std::fs;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::Builder::new().tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        Files::shut_others_out(temp.path());
        let home = dunce::canonicalize(temp.path()).unwrap().join("home");
        Files::create_private_dir(&home).unwrap();
        (temp, home)
    }

    fn directory(path: &Path) {
        Files::create_private_dir(path).unwrap();
    }

    fn put(path: &Path) {
        Files::create_private(path, b"preserve outside owned names").unwrap();
    }

    fn remove_entry(home: &Path, path: &Path) -> Result<(), &'static str> {
        Home::open(home).remove_entry(path)
    }

    /// macOS as `dirs`, Tauri's common rule, `data_dir` and `webview_data` resolve it for `home`: the logs that rule
    /// names are inside the app's data folder (macOS's own `~/Library/Logs/<id>` is one of its extras).
    pub(super) fn macos(home: &Path) -> Places {
        let support = home.join("Library/Application Support");
        Places {
            data: support.join(IDENTIFIER),
            cache: home.join("Library/Caches").join(IDENTIFIER),
            logs: support.join(IDENTIFIER).join("logs"),
            models: support.join("Sidevoice").join(sidevoice_platform::data_dir::MODELS),
            webview: vec![
                support.join(IDENTIFIER),
                home.join("Library/WebKit").join(IDENTIFIER),
                home.join("Library/Caches").join(IDENTIFIER),
            ],
        }
    }

    /// [`macos`] with the logs in a folder of their own, as on Windows (`<local data>/<id>/logs`).
    fn separate_logs(home: &Path) -> Places {
        Places { logs: home.join("AppData/Local").join(IDENTIFIER).join("logs"), ..macos(home) }
    }

    pub(super) fn named(folders: &Folders) -> Vec<(&'static str, PathBuf)> {
        folders.list.iter().map(|folder| (folder.name, folder.path.clone())).collect()
    }

    fn run(
        home: &Path,
        folders: &Folders,
        browser: &io::Result<Released>,
    ) -> Vec<(&'static str, Result<(), &'static str>)> {
        let mut seen = Vec::new();
        assert!(folders
            .delete(&Home::open(home), browser, &mut |name, result| {
                seen.push((name, result));
                true
            })
            .is_ok());
        seen
    }

    fn busy() -> io::Result<Released> {
        Err(io::Error::from(io::ErrorKind::WouldBlock))
    }

    /// The common folders on macOS; the macOS extras (`app_data/macos.rs`) follow them with `logs`, `saved-state`
    /// and `preferences`, which together are the previous deletion set and order verbatim.
    #[test]
    fn the_common_macos_folders_are_the_previous_list_before_its_extras() {
        let home = Path::new("/Users/ana");
        let models = home.join("Library/Application Support/Sidevoice").join(sidevoice_platform::data_dir::MODELS);
        assert_eq!(
            named(&Folders::from(macos(home))),
            vec![
                ("app-data", home.join("Library/Application Support/dev.sidevoice.desktop")),
                ("models", models.clone()),
                ("webview", home.join("Library/WebKit/dev.sidevoice.desktop")),
                ("cache", home.join("Library/Caches/dev.sidevoice.desktop")),
            ]
        );
        assert_eq!(models, Path::new("/Users/ana/Library/Application Support/Sidevoice/Models"));
        assert_eq!(MODEL_CHILDREN, ["blobs", "models", "partial"]);
    }

    #[test]
    fn an_alternate_home_is_refused_before_anything_is_deleted() {
        let (_temp, home) = fixture();
        // A canonical, private, same-owner home restored inside the real one, supplied as `HOME`: never the home
        // the account record names.
        let alternate = home.join("restored-account-home");
        directory(&alternate);
        let places = separate_logs(&alternate);
        for path in [&places.data, &places.logs, &places.models.join("blobs")] {
            for ancestor in
                path.ancestors().collect::<Vec<_>>().into_iter().rev().filter(|dir| dir.starts_with(&alternate))
            {
                if !ancestor.exists() {
                    directory(ancestor);
                }
            }
            put(&path.join("payload"));
        }
        let mut seen = Vec::new();
        let deleted = delete_data(&alternate, &busy(), &mut |name, result| {
            seen.push((name, result));
            true
        });
        assert!(matches!(deleted, Err(Error::Failed("unsafe-home"))));
        assert_eq!(seen, vec![("app-data", Err("unsafe-home"))]);
        for path in [&places.data, &places.logs, &places.models.join("blobs")] {
            assert!(path.join("payload").is_file());
        }
        // The account's own home is accepted and its folders go.
        assert!(
            delete_folders(&Home::open(&alternate), Ok(separate_logs(&alternate)), &busy(), &mut |_, _| true).is_ok()
        );
        assert!(!places.logs.exists() && !places.models.join("blobs").exists());
    }

    #[test]
    fn a_folder_inside_another_goes_with_it_and_shared_places_are_named_once() {
        let home = Path::new("/home/ana");
        let local = home.join(".local/share");
        // Linux: data, local data and the browser are one folder, holding the logs.
        let folders = Folders::from(Places {
            data: local.join(IDENTIFIER),
            cache: home.join(".cache").join(IDENTIFIER),
            logs: local.join(IDENTIFIER).join("logs"),
            models: local.join("sidevoice/Models"),
            webview: vec![local.join(IDENTIFIER)],
        });
        assert_eq!(
            named(&folders),
            vec![
                ("app-data", local.join(IDENTIFIER)),
                ("models", local.join("sidevoice/Models")),
                ("cache", home.join(".cache").join(IDENTIFIER)),
            ]
        );
    }

    #[test]
    fn native_folders_are_inside_home_distinct_and_unnested() {
        let Ok(home) = account::home() else { return };
        let Ok(folders) = Places::native(home.clone()).map(Folders::from) else { return };
        for (index, folder) in folders.list.iter().enumerate() {
            assert!(folder.path.starts_with(&home) && folder.path != home);
            assert!(folders.list[index + 1..]
                .iter()
                .all(|other| !other.path.starts_with(&folder.path) && !folder.path.starts_with(&other.path)));
        }
        assert!(folders.list.iter().any(|folder| folder.name == "models" && folder.path == folders.models));
    }

    #[test]
    fn only_the_folders_go_and_unreleased_browser_storage_stays() {
        let (_temp, home) = fixture();
        let places = separate_logs(&home);
        for path in [&places.data, &places.webview[1], &places.cache, &places.logs, &places.models] {
            for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev().filter(|dir| dir.starts_with(&home))
            {
                if !ancestor.exists() {
                    directory(ancestor);
                }
            }
            put(&path.join("payload"));
        }
        for child in MODEL_CHILDREN {
            directory(&places.models.join(child));
        }
        directory(&places.models.join("other-program"));
        put(&home.join("Library/unrelated"));
        let folders = Folders::from(places);
        let seen = run(&home, &folders, &busy());
        let held = Err("reset.process-busy");
        assert_eq!(
            seen,
            vec![("app-data", held), ("models", Ok(())), ("webview", held), ("cache", held), ("logs", Ok(()))]
        );
        assert!(folders.list[0].path.join("payload").is_file() && folders.list[2].path.join("payload").is_file());
        assert!(!folders.list[4].path.exists());
        assert!(MODEL_CHILDREN.iter().all(|child| !folders.models.join(child).exists()));
        assert!(folders.models.join("other-program").is_dir() && folders.models.join("payload").is_file());
        assert!(home.join("Library/unrelated").is_file());
        // Missing folders are completed steps.
        assert_eq!(run(&home, &folders, &busy())[4], ("logs", Ok(())));
    }

    #[test]
    fn a_stop_ends_the_deletion() {
        let (_temp, home) = fixture();
        let folders = Folders::from(macos(&home));
        let mut seen = 0;
        assert!(matches!(
            folders.delete(&Home::open(&home), &busy(), &mut |_, _| {
                seen += 1;
                false
            }),
            Err(Error::Stopped)
        ));
        assert_eq!(seen, 1);
    }

    #[test]
    fn links_are_unlinked_never_followed_and_hard_links_keep_their_data() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        directory(&outside);
        put(&outside.join("sentinel"));
        let tree = home.join("state");
        directory(&tree);
        Files::symlink(&outside, tree.join("directory-link"));
        Files::symlink(outside.join("sentinel"), tree.join("file-link"));
        Files::symlink(outside.join("missing"), tree.join("dangling"));
        fs::hard_link(outside.join("sentinel"), tree.join("hard")).unwrap();
        // A shell shortcut is an ordinary file, irrespective of the platform running this fixture.
        put(&tree.join("shortcut.lnk"));
        assert_eq!(remove_entry(&home, &tree), Ok(()));
        assert!(!tree.exists());
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve outside owned names");
        Files::symlink(&outside, &tree);
        assert_eq!(remove_entry(&home, &tree), Ok(()));
        assert!(fs::symlink_metadata(&tree).is_err() && outside.join("sentinel").is_file());
    }

    #[test]
    fn every_model_child_is_preflighted_before_any_is_deleted_and_links_are_unlinked() {
        let (temp, home) = fixture();
        let mut places = macos(&home);
        places.models = home.join("store");
        let folders = Folders::from(places);
        let outside = temp.path().join("outside");
        directory(&outside);
        put(&outside.join("sentinel"));
        directory(&folders.models);
        Files::symlink(&outside, folders.models.join("blobs"));
        for child in ["models", "partial"] {
            directory(&folders.models.join(child));
            put(&folders.models.join(child).join("payload"));
        }
        Files::let_others_change(&folders.models.join("partial"));
        assert!(folders.remove_models(&Home::open(&home)).is_err());
        assert!(folders.models.join("models/payload").is_file());
        Files::shut_others_out(&folders.models.join("partial"));
        assert_eq!(folders.remove_models(&Home::open(&home)), Ok(()));
        assert!(MODEL_CHILDREN.iter().all(|child| fs::symlink_metadata(folders.models.join(child)).is_err()));
        assert!(outside.join("sentinel").is_file());
    }

    #[test]
    fn links_between_home_and_the_parent_and_outside_paths_are_refused() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        directory(&outside);
        directory(&outside.join("state"));
        put(&outside.join("state/sentinel"));
        Files::symlink(&outside, home.join("linked"));
        assert_eq!(remove_entry(&home, &home.join("linked/state")), Err("unsafe-directory"));
        // A redirected model store is refused, not traversed.
        let mut places = macos(&home);
        places.models = home.join("linked");
        directory(&outside.join("blobs"));
        assert!(Folders::from(places).remove_models(&Home::open(&home)).is_err());
        assert!(outside.join("blobs").is_dir());
        let alias = temp.path().join("home-alias");
        Files::symlink(&home, &alias);
        directory(&home.join("state"));
        assert_eq!(remove_entry(&alias, &alias.join("state")), Err("unsafe-home"));
        for refused in [&home, &temp.path().join("other"), &home.join("state/../x"), &PathBuf::from("relative")] {
            assert!(remove_entry(&home, refused).is_err());
        }
        assert!(outside.join("state/sentinel").is_file() && home.join("state").is_dir());
    }

    #[test]
    fn the_read_only_check_refuses_a_linked_protected_root_and_deletes_nothing() {
        let (temp, home) = fixture();
        let tree = home.join("data");
        directory(&tree);
        put(&tree.join("payload"));
        Files::symlink(temp.path().join("absent"), tree.join("link"));
        assert_eq!(check_tree(&home, &tree), Ok(()));
        assert_eq!(check_tree(&home, &home.join("missing")), Ok(()));
        Files::symlink(&tree, home.join("alias"));
        assert!(check_tree(&home, &home.join("alias")).is_err());
        assert!(check_tree(&home, temp.path()).is_err());
        assert!(tree.join("payload").is_file());
    }

    /// The ordinary layout under `root`: `logs/payload` and the model store's children, each with a payload.
    fn outside_layout(root: &Path) {
        directory(root);
        directory(&root.join("logs"));
        put(&root.join("logs/payload"));
        for child in MODEL_CHILDREN {
            directory(&root.join(child));
            put(&root.join(child).join("payload"));
        }
    }

    fn outside_intact(root: &Path) -> bool {
        root.join("logs/payload").is_file()
            && MODEL_CHILDREN.iter().all(|child| root.join(child).join("payload").is_file())
    }

    #[test]
    fn a_link_at_a_deletion_root_or_at_an_ancestor_is_refused_and_its_target_stays() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        outside_layout(&outside);
        let places = separate_logs(&home);
        // An ancestor of the logs: `AppData` leads outside, where `Local/<id>/logs` exists too.
        directory(&outside.join("Local"));
        directory(&outside.join("Local").join(IDENTIFIER));
        fs::rename(outside.join("logs"), outside.join("Local").join(IDENTIFIER).join("logs")).unwrap();
        Files::symlink(&outside, home.join("AppData"));
        // The model store itself leads outside, where the engine-owned children exist.
        directory(&home.join("Library"));
        directory(&home.join("Library/Application Support"));
        directory(&home.join("Library/Application Support/Sidevoice"));
        Files::symlink(&outside, &places.models);
        let mut seen = Vec::new();
        let _ = delete_folders(&Home::open(&home), Ok(places), &busy(), &mut |name, result| {
            seen.push((name, result));
            true
        });
        let busy_key = Err("reset.process-busy");
        assert_eq!(
            seen,
            vec![
                ("app-data", busy_key),
                ("models", Err("unsafe-directory")),
                ("webview", busy_key),
                ("cache", busy_key),
                ("logs", Err("unsafe-directory")),
            ]
        );
        assert!(outside.join("Local").join(IDENTIFIER).join("logs/payload").is_file());
        assert!(MODEL_CHILDREN.iter().all(|child| outside.join(child).join("payload").is_file()));
        assert!(fs::symlink_metadata(home.join("AppData")).unwrap().file_type().is_symlink());
    }

    /// The race the held home closes, made deterministic: the home is checked and held first, then a deletion root
    /// and an ancestor are replaced by links before the deletion reaches them.
    #[test]
    fn links_swapped_in_after_the_home_is_held_are_refused_and_their_targets_stay() {
        let (temp, home) = fixture();
        let outside = temp.path().join("outside");
        outside_layout(&outside);
        let store = home.join("store");
        let state = home.join("state");
        outside_layout(&store);
        outside_layout(&state);
        let mut places = macos(&home);
        places.models = store.clone();
        let folders = Folders::from(places);
        let held = Home::open(&home);
        // Validated and held; now swap the store (a deletion root) and `state` (an ancestor of `state/logs`).
        fs::rename(&store, home.join("store-moved")).unwrap();
        Files::symlink(&outside, &store);
        fs::rename(&state, home.join("state-moved")).unwrap();
        Files::symlink(&outside, &state);
        assert_eq!(folders.remove_models(&held), Err("unsafe-directory"));
        assert_eq!(held.remove_entry(&state.join("logs").join("payload")), Err("unsafe-directory"));
        assert_eq!(held.remove_entry(&state.join("logs")), Err("unsafe-directory"));
        assert_eq!(check_tree(&home, &state.join("logs")), Err("unsafe-directory"));
        assert_eq!(check_tree(&home, &store), Err("unsafe-directory"));
        assert!(outside_intact(&outside));
        assert!(outside_intact(&home.join("store-moved")) && outside_intact(&home.join("state-moved")));
        // A link that is itself the selected entry is unlinked, never followed.
        assert_eq!(held.remove_entry(&state), Ok(()));
        assert!(fs::symlink_metadata(&state).is_err() && outside_intact(&outside));
    }
}
