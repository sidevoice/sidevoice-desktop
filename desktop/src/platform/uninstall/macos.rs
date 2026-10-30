//! macOS: the app removes its own bundle to the Trash, where the person can put it back from.

use super::{Error, Os};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) struct Native;

impl Os for Native {
    const AVAILABLE: bool = true;

    /// This copy's bundle, checked, then Finder's move to the Trash.
    fn uninstall() -> Result<(), Error> {
        removable_app().and_then(|app| trash(&app)).map_err(Error::Failed)
    }
}

/// This app's own bundle: a real directory under its canonical path, with its Info.plist, in a folder this
/// user may write.
fn removable_app() -> Result<PathBuf, &'static str> {
    use std::os::unix::ffi::OsStrExt;
    let exe = std::env::current_exe().and_then(fs::canonicalize).map_err(|_| "reset.app-unremovable")?;
    let bundle = app_bundle_of(&exe)?;
    let meta = fs::symlink_metadata(&bundle).map_err(|_| "reset.app-unremovable")?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || fs::canonicalize(&bundle).ok().as_deref() != Some(bundle.as_path())
        || !bundle.join("Contents/Info.plist").is_file()
    {
        return Err("reset.app-unremovable");
    }
    let parent = bundle.parent().ok_or("reset.app-unremovable")?;
    let parent = std::ffi::CString::new(parent.as_os_str().as_bytes()).map_err(|_| "reset.app-unremovable")?;
    if unsafe { libc::access(parent.as_ptr(), libc::W_OK) } != 0 {
        return Err("reset.app-unremovable");
    }
    Ok(bundle)
}

/// Finder's own move to the Trash (`NSFileManager trashItemAtURL:`), so the app can be put back from there.
fn trash(app: &Path) -> Result<(), &'static str> {
    use objc2::{class, msg_send, rc::autoreleasepool, runtime::AnyObject};
    use objc2_foundation::NSString;
    let text = app.to_str().ok_or("reset.app-unremovable")?;
    let moved = autoreleasepool(|_| unsafe {
        let manager: *mut AnyObject = msg_send![class!(NSFileManager), defaultManager];
        let path = NSString::from_str(text);
        let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: &*path, isDirectory: true];
        if manager.is_null() || url.is_null() {
            return false;
        }
        let resulting: *mut *mut AnyObject = std::ptr::null_mut();
        let error: *mut *mut AnyObject = std::ptr::null_mut();
        let moved: bool = msg_send![manager, trashItemAtURL: url, resultingItemURL: resulting, error: error];
        moved
    });
    if moved {
        Ok(())
    } else {
        Err("reset.app-trash-failed")
    }
}

/// The `.app` bundle this executable runs from: `<bundle>.app/Contents/MacOS/<executable>`. Anything else (a
/// development build, a path with `..`, a translocated copy) has no bundle the uninstall may remove.
fn app_bundle_of(exe: &Path) -> Result<PathBuf, &'static str> {
    let clean =
        exe.is_absolute() && exe.components().all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
    if !clean || exe.components().any(|part| part.as_os_str() == "AppTranslocation") {
        return Err("reset.app-unremovable");
    }
    let macos = exe.parent().filter(|dir| dir.file_name().is_some_and(|name| name == "MacOS"));
    let contents = macos.and_then(Path::parent).filter(|dir| dir.file_name().is_some_and(|name| name == "Contents"));
    let bundle = contents.and_then(Path::parent).filter(|dir| {
        dir.extension().is_some_and(|ext| ext == "app") && dir.parent().is_some_and(|parent| parent != Path::new("/"))
    });
    bundle.map(Path::to_path_buf).ok_or("reset.app-unremovable")
}

/// The bundle layout is macOS's own: there is no host-agnostic phrasing of these cases.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_real_app_bundle_can_be_removed() {
        let ok = |exe: &str| app_bundle_of(Path::new(exe));
        assert_eq!(
            ok("/Applications/Sidevoice.app/Contents/MacOS/sidevoice-desktop"),
            Ok(PathBuf::from("/Applications/Sidevoice.app"))
        );
        assert_eq!(
            ok("/Users/ana/Applications/Sidevoice Nightly.app/Contents/MacOS/sidevoice-desktop"),
            Ok(PathBuf::from("/Users/ana/Applications/Sidevoice Nightly.app"))
        );
        for refused in [
            // A development build has no bundle.
            "/Users/ana/sidevoice-desktop/target/debug/sidevoice-desktop",
            "/Applications/Sidevoice/Contents/MacOS/sidevoice-desktop",
            "/Applications/Sidevoice.app/Contents/Resources/sidevoice-desktop",
            "/Applications/Sidevoice.app/MacOS/sidevoice-desktop",
            "/Sidevoice.app/Contents/MacOS/sidevoice-desktop",
            "/Applications/../Sidevoice.app/Contents/MacOS/sidevoice-desktop",
            "Sidevoice.app/Contents/MacOS/sidevoice-desktop",
            "/private/var/folders/x/T/AppTranslocation/ABC/d/Sidevoice.app/Contents/MacOS/sidevoice-desktop",
        ] {
            assert_eq!(ok(refused), Err("reset.app-unremovable"), "{refused}");
        }
    }
}
