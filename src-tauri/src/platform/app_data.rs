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
//! Every removal goes through the shared held-root capability (`HeldDirectory` + `Removal`): the selected entry and
//! its whole tree are preflighted before anything is deleted, links are unlinked and never followed, entry identity
//! and mounts below the held parent are checked while deleting. That capability checks mounts only from the held
//! parent down, so until it covers the rest this module refuses, between the account home and that parent, any
//! link (the path must be its own canonical form) and any change of volume, and a home that is a volume's root.

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
    delete_app_folders(home, browser, steps)?;
    Native::delete_extra_data(home, steps)
}

/// Read-only: whether an existing protected directory under `home` (the connector's data and releases) and its
/// whole tree could be removed. It must be an ordinary directory, reached from `home` with no link or volume
/// change. A missing directory passes. The `Err` is a short reason.
pub fn check_tree(home: &Path, path: &Path) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("unsafe-path")?;
    let name = path.file_name().ok_or("unsafe-path")?;
    if Ancestry::check(home, path)?.is_none() {
        return Ok(());
    }
    match Ancestry::check(home, parent)? {
        Some(held) => held.check_contents(&[name]).map_err(|_| "unsafe-entry"),
        None => Ok(()),
    }
}

/// What the OS keeps for the app beyond its folders. Hosts with nothing more keep the default.
trait Os {
    /// Delete it inside the account home `home`, reporting each step to `steps`.
    fn delete_extra_data(_home: &Path, _steps: Steps) -> Result<(), Error> {
        Ok(())
    }
}

/// The app folders every OS has, once `home` is checked to still be the account home.
fn delete_app_folders(home: &Path, browser: &io::Result<Released>, steps: Steps) -> Result<(), Error> {
    let places = match account::home() {
        Ok(account) if account == home => Places::native(account),
        _ => Err("unsafe-home"),
    };
    delete_folders(places, browser, steps)
}

/// Delete the folders of `places`, or report why there are none before deleting anything.
fn delete_folders(
    places: Result<Places, &'static str>,
    browser: &io::Result<Released>,
    steps: Steps,
) -> Result<(), Error> {
    match places {
        Ok(places) => Folders::from(places).delete(browser, steps),
        Err(key) => {
            steps("app-data", Err(key));
            Err(Error::Failed(key))
        }
    }
}

/// Remove one entry the app owns (not browser storage, not the model store) inside the account home `home`.
fn remove_entry(home: &Path, path: &Path) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("unsafe-path")?;
    let name = path.file_name().ok_or("unsafe-path")?;
    match Ancestry::check(home, parent)? {
        Some(held) => held.remove_contents(&[name]).map_err(|_| "remove"),
        None => Ok(()),
    }
}

/// The places Tauri, the model store and the browser storage resolve to for this account.
struct Places {
    home: PathBuf,
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
    home: PathBuf,
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
            home,
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
        Self { home: places.home, models: places.models, webview: places.webview, list }
    }

    fn delete(&self, browser: &io::Result<Released>, steps: Steps) -> Result<(), Error> {
        for folder in &self.list {
            let result = if self.webview.contains(&folder.path) {
                match browser {
                    Ok(released) => check_tree(&self.home, &folder.path)
                        .and_then(|_| webview_data::remove(released, &folder.path).map_err(|_| "remove")),
                    Err(_) => Err("reset.process-busy"),
                }
            } else if folder.path == self.models {
                self.remove_models()
            } else {
                remove_entry(&self.home, &folder.path)
            };
            if !steps(folder.name, result) {
                return Err(Error::Stopped);
            }
        }
        Ok(())
    }

    /// The engine-owned children only: the shared capability preflights all of them before removing any. The
    /// store itself must be an ordinary directory; a child may be a link, which is unlinked.
    fn remove_models(&self) -> Result<(), &'static str> {
        match Ancestry::check(&self.home, &self.models)? {
            Some(held) => held.remove_contents(&MODEL_CHILDREN.map(OsStr::new)).map_err(|_| "remove"),
            None => Ok(()),
        }
    }
}

/// The interim check between the account home and a held parent (see the module docs).
struct Ancestry;

impl Ancestry {
    fn clean_absolute(path: &Path) -> bool {
        path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir | Component::Normal(_)))
    }

    /// The volume a path is on; links are followed, which the canonical-path check excludes.
    fn volume(path: &Path) -> io::Result<u64> {
        Ok(match file_id::get_file_id(path)? {
            file_id::FileId::Inode { device_id, .. } => device_id,
            file_id::FileId::LowRes { volume_serial_number, .. } => volume_serial_number.into(),
            file_id::FileId::HighRes { volume_serial_number, .. } => volume_serial_number,
        })
    }

    /// Hold `dir` (inside or equal to `home`) after checking that `home` is its own canonical path, is not a
    /// volume's root, and that every directory from `home` to `dir` is its own canonical path on home's volume.
    /// A missing directory on the way is `None`.
    fn check(home: &Path, dir: &Path) -> Result<Option<HeldDirectory>, &'static str> {
        if !Self::clean_absolute(home)
            || !Self::clean_absolute(dir)
            || home.components().filter(|part| matches!(part, Component::Normal(_))).count() < 2
        {
            return Err("unsafe-path");
        }
        let relative = dir.strip_prefix(home).map_err(|_| "outside-home")?;
        if dunce::canonicalize(home).ok().as_deref() != Some(home) {
            return Err("unsafe-home");
        }
        let expected = Self::volume(home).map_err(|_| "unsafe-home")?;
        let outer = home.parent().ok_or("unsafe-home")?;
        if Self::volume(outer).map_err(|_| "inspect-mount")? != expected {
            return Err("volume-root");
        }
        let mut prefix = home.to_path_buf();
        for part in relative.components() {
            let Component::Normal(name) = part else { return Err("unsafe-path") };
            prefix.push(name);
            match dunce::canonicalize(&prefix) {
                Ok(real) if real == prefix => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                _ => return Err("unsafe-directory"),
            }
            if Self::volume(&prefix).map_err(|_| "inspect-mount")? != expected {
                return Err("mount-boundary");
            }
        }
        match HeldDirectory::open(dir, Access::NotWritableByOthers) {
            Ok(held) => Ok(Some(held)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("unsafe-directory"),
        }
    }
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

    /// macOS as `dirs`, Tauri's common rule, `data_dir` and `webview_data` resolve it for `home`: the logs that rule
    /// names are inside the app's data folder (macOS's own `~/Library/Logs/<id>` is one of its extras).
    pub(super) fn macos(home: &Path) -> Places {
        let support = home.join("Library/Application Support");
        Places {
            home: home.to_path_buf(),
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

    fn run(folders: &Folders, browser: &io::Result<Released>) -> Vec<(&'static str, Result<(), &'static str>)> {
        let mut seen = Vec::new();
        assert!(folders
            .delete(browser, &mut |name, result| {
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
        assert!(delete_folders(Ok(separate_logs(&alternate)), &busy(), &mut |_, _| true).is_ok());
        assert!(!places.logs.exists() && !places.models.join("blobs").exists());
    }

    #[test]
    fn a_folder_inside_another_goes_with_it_and_shared_places_are_named_once() {
        let home = Path::new("/home/ana");
        let local = home.join(".local/share");
        // Linux: data, local data and the browser are one folder, holding the logs.
        let folders = Folders::from(Places {
            home: home.to_path_buf(),
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
        let Ok(folders) = account::home().and_then(Places::native).map(Folders::from) else { return };
        for (index, folder) in folders.list.iter().enumerate() {
            assert!(folder.path.starts_with(&folders.home) && folder.path != folders.home);
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
        let seen = run(&folders, &busy());
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
        assert_eq!(run(&folders, &busy())[4], ("logs", Ok(())));
    }

    #[test]
    fn a_stop_ends_the_deletion() {
        let (_temp, home) = fixture();
        let folders = Folders::from(macos(&home));
        let mut seen = 0;
        assert!(matches!(
            folders.delete(&busy(), &mut |_, _| {
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
        assert!(folders.remove_models().is_err());
        assert!(folders.models.join("models/payload").is_file());
        Files::shut_others_out(&folders.models.join("partial"));
        assert_eq!(folders.remove_models(), Ok(()));
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
        assert!(Folders::from(places).remove_models().is_err());
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
}
