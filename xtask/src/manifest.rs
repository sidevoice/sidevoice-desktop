//! `manifest DIR [--tag vX.Y.Z]`: every target's installer in DIR, under its published name, and `SHA256SUMS`.
//!
//! A release keeps tauri's names (`Sidevoice_X.Y.Z_aarch64.dmg`, …); the `nightly` snapshot gets fixed names
//! (`Sidevoice_nightly_aarch64.dmg`, …), so a link to it keeps working while the app inside reports the version of the
//! last release (RELEASING.md).

use crate::util::version;
use crate::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// One installer per kind: the targets a release ships.
pub const KINDS: &[&str] = &[".dmg", "-setup.exe", ".deb", ".AppImage"];
pub const SUMS: &str = "SHA256SUMS";

pub fn manifest(dir: &Path, tag: Option<&str>) -> Result<()> {
    let version = version()?;
    if let Some(tag) = tag {
        if tag != format!("v{version}") {
            return Err(format!("the release is {tag}, but the app's version is {version}"));
        }
    }
    let mut assets = Vec::new();
    for kind in KINDS {
        let found: Vec<PathBuf> = files(dir)?.into_iter().filter(|p| name(p).ends_with(kind)).collect();
        let [file] = found.as_slice() else {
            return Err(format!("expected one *{kind} in {}, found {found:?}", dir.display()));
        };
        let published =
            if tag.is_some() { name(file) } else { name(file).replace(&format!("_{version}_"), "_nightly_") };
        if !published.starts_with("Sidevoice_") {
            return Err(format!("{} is not a Sidevoice installer", file.display()));
        }
        let to = dir.join(&published);
        if *file != to {
            fs::rename(file, &to).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        assets.push(to);
    }
    let mut sums = String::new();
    for asset in &assets {
        sums += &format!("{}  {}\n", sha256(asset)?, name(asset));
    }
    fs::write(dir.join(SUMS), &sums).map_err(|e| e.to_string())?;
    eprint!("{sums}");
    let unexpected: Vec<String> =
        files(dir)?.iter().map(|p| name(p)).filter(|n| n != SUMS && !assets.iter().any(|a| name(a) == *n)).collect();
    if !unexpected.is_empty() {
        return Err(format!("{} holds files that are not assets: {unexpected:?}", dir.display()));
    }
    Ok(())
}

pub fn files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    Ok(files)
}

pub fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
