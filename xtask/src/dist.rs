//! `dist`: this host's installers, built and checked as a release ships them, into `target/dist/`.

use crate::util::{fresh_dir, target};
use crate::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub fn dist() -> Result<()> {
    let out = target().join("dist");
    fresh_dir(&out)?;
    let made = build(&out)?;
    for file in &made {
        eprintln!("dist: {}", file.display());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn build(out: &Path) -> Result<Vec<PathBuf>> {
    use crate::macos;
    use crate::util::Checks;
    macos::build(None)?;
    let mut checks = Checks::default();
    macos::verify_app(&macos::app(), &mut checks)?;
    macos::first_open(&mut checks)?;
    macos::with_target(&mut checks)?;
    checks.done("the macOS app")?;
    Ok(vec![macos::dmg(&macos::app(), out)?])
}

#[cfg(target_os = "linux")]
fn build(out: &Path) -> Result<Vec<PathBuf>> {
    crate::util::run(&mut crate::util::tauri(&["build", "--bundles", "deb,appimage"]))?;
    let bundle = target().join("release/bundle");
    collect(&[(bundle.join("deb"), ".deb"), (bundle.join("appimage"), ".AppImage")], out)
}

#[cfg(windows)]
fn build(out: &Path) -> Result<Vec<PathBuf>> {
    crate::util::run(&mut crate::util::tauri(&["build", "--bundles", "nsis"]))?;
    collect(&[(target().join("release/bundle/nsis"), "-setup.exe")], out)
}

/// Copies the one file of each kind (directory, name suffix) into `out`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn collect(kinds: &[(PathBuf, &str)], out: &Path) -> Result<Vec<PathBuf>> {
    let mut made = Vec::new();
    for (dir, suffix) in kinds {
        let found: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.is_file() && path.to_string_lossy().ends_with(suffix))
            .collect();
        let [file] = found.as_slice() else {
            return Err(format!("expected one *{suffix} in {}, found {found:?}", dir.display()));
        };
        let to = out.join(file.file_name().expect("a file"));
        fs::copy(file, &to).map_err(|e| format!("{}: {e}", file.display()))?;
        made.push(to);
    }
    Ok(made)
}
