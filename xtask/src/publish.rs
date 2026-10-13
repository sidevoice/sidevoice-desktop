//! `publish …`: a run's installers onto GitHub (RELEASING.md): the release that called the build, or the rolling
//! `nightly` pre-release. `gh`, with `GH_TOKEN` and `GH_REPO` from the environment.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::util::{ensure, files, name, repo, version, write, Cmd};
use crate::Result;

/// The installers a run publishes, by how their names end; and the kinds (extensions) every publication must hold
/// one of at least.
const INSTALLERS: &[&str] = &[".dmg", "-setup.exe", ".deb", ".AppImage"];
const KINDS: &[&str] = &["dmg", "exe", "deb", "AppImage"];
const SUMS: &str = "SHA256SUMS";

/// Where the build's artifacts are downloaded to, and where the assets are put together.
fn artifacts() -> PathBuf {
    repo().join("artifacts")
}

fn assets() -> PathBuf {
    repo().join("assets")
}

pub fn is_installer(file: &str) -> bool {
    INSTALLERS.iter().any(|end| file.ends_with(end))
}

/// The name an installer is published under: as built for a release; for the nightly, `_nightly_` where its version
/// was (`Sidevoice_0.3.0_aarch64.dmg` → `Sidevoice_nightly_aarch64.dmg`), so a link to it keeps working.
pub fn published_name(file: &str, version: &str, nightly: bool) -> Result<String> {
    if !nightly {
        return Ok(file.to_owned());
    }
    let built = format!("_{version}_");
    ensure(file.contains(&built), format!("{file} does not carry the version {version}"))?;
    Ok(file.replacen(&built, "_nightly_", 1))
}

/// The kinds of installer missing from `names`.
pub fn missing_kinds(names: &[String]) -> Vec<&'static str> {
    KINDS.iter().copied().filter(|kind| !names.iter().any(|name| name.ends_with(&format!(".{kind}")))).collect()
}

/// `sha256sum` of `paths`: one `<hex>  <name>` line each.
pub fn sums(paths: &[PathBuf]) -> Result<String> {
    let mut out = String::new();
    for path in paths {
        let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(|e| format!("{}: {e}", path.display()))?;
        out += &format!("{:x}  {}\n", hasher.finalize(), name(path));
    }
    Ok(out)
}

/// Every file under `dir`, its subdirectories included (`find DIR -type f`).
fn all_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            found.extend(all_files(&path)?);
        } else if path.is_file() {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// `publish assets release|nightly`: the installers among the build's artifacts (`artifacts/`, as
/// actions/download-artifact leaves them) into `assets/`, under their published names, each kind there, and their
/// `SHA256SUMS`.
pub fn assemble(kind: &str) -> Result<()> {
    let nightly = match kind {
        "release" => false,
        "nightly" => true,
        _ => return Err(format!("publish assets: release or nightly, not {kind}")),
    };
    let version = version()?;
    let assets = assets();
    // As `mkdir assets` did: never over an earlier one.
    ensure(!assets.exists(), format!("{} is there already: remove it first", assets.display()))?;
    crate::util::mkdir(&assets)?;
    for file in all_files(&artifacts())? {
        let built = name(&file);
        if is_installer(&built) {
            let to = assets.join(published_name(&built, &version, nightly)?);
            fs::copy(&file, &to).map_err(|e| format!("{}: {e}", file.display()))?;
        }
    }
    let present: Vec<PathBuf> = files(&assets)?;
    let names: Vec<String> = present.iter().map(|p| name(p)).collect();
    let missing = missing_kinds(&names);
    ensure(missing.is_empty(), format!("no {} among the artifacts", missing.join(", ")))?;
    let sums = sums(&present)?;
    write(&assets.join(SUMS), &sums)?;
    Cmd::new("ls").arg("-la").arg(&assets).run()?;
    eprint!("{sums}");
    Ok(())
}

/// The files to upload: everything in `assets/`.
fn uploads() -> Result<Vec<PathBuf>> {
    let uploads = files(&assets())?;
    ensure(
        uploads.iter().any(|p| name(p) == SUMS),
        format!("assets/ has no {SUMS}: run `cargo xtask publish assets` first"),
    )?;
    Ok(uploads)
}

/// How `gh release edit` publishes the draft TAG: a version with a `-` suffix (`-rc.N`) is a pre-release, never
/// latest.
pub fn release_flags(tag: &str) -> [&'static str; 3] {
    if tag.contains('-') {
        ["--draft=false", "--prerelease", "--latest=false"]
    } else {
        ["--draft=false", "--prerelease=false", "--latest"]
    }
}

/// `publish release TAG`: `assets/` attached to the draft release TAG (release-please's), then published.
pub fn release(tag: &str) -> Result<()> {
    let uploads = uploads()?;
    Cmd::new("gh").args(["release", "upload", tag]).args(&uploads).arg("--clobber").run()?;
    Cmd::new("gh").args(["release", "edit", tag]).args(release_flags(tag)).run()
}

/// The nightly's notes.
pub fn nightly_notes(sha: &str, when: &str) -> String {
    format!(
        "Snapshot of `main` at {sha} (committed {when}). Not a versioned release: the `nightly` tag moves to every \
         commit on `main` whose CI passes, and these assets are replaced each time. Versioned releases are the \
         `vX.Y.Z` ones."
    )
}

/// The assets of the release that `local` does not replace: an older snapshot's leftovers.
pub fn stale<'a>(published: &'a str, local: &[String]) -> Vec<&'a str> {
    published.lines().filter(|name| !name.is_empty() && !local.iter().any(|l| l == name)).collect()
}

/// `publish nightly`: the tag `nightly` moved to this commit (`GITHUB_SHA`, or HEAD), and the one `nightly`
/// pre-release made of `assets/`: its notes and every asset replaced, an older snapshot's leftovers removed; created
/// the first time. `WHEN` (the commit's time; CI gives the push's) goes into its notes.
pub fn nightly() -> Result<()> {
    let uploads = uploads()?;
    let repo_name = std::env::var("GH_REPO").map_err(|_| "GH_REPO is not set (owner/name)")?;
    let sha = match std::env::var("GITHUB_SHA") {
        Ok(sha) if !sha.is_empty() => sha,
        _ => Cmd::new("git").args(["rev-parse", "HEAD"]).stdout()?.trim().to_owned(),
    };
    let when = match std::env::var("WHEN") {
        Ok(when) if !when.is_empty() => when,
        _ => Cmd::new("git").args(["show", "-s", "--format=%cI", &sha]).stdout()?.trim().to_owned(),
    };
    let quiet = |cmd: Cmd| cmd.stdout().map(drop);
    let (exists, _) = Cmd::new("gh").args(["api", &format!("repos/{repo_name}/git/ref/tags/nightly")]).attempt()?;
    if exists {
        quiet(
            Cmd::new("gh").args(["api", "-X", "PATCH", &format!("repos/{repo_name}/git/refs/tags/nightly")]).args([
                "-f",
                &format!("sha={sha}"),
                "-F",
                "force=true",
            ]),
        )?;
    } else {
        quiet(Cmd::new("gh").args(["api", "-X", "POST", &format!("repos/{repo_name}/git/refs")]).args([
            "-f",
            "ref=refs/tags/nightly",
            "-f",
            &format!("sha={sha}"),
        ]))?;
    }
    let notes = nightly_notes(&sha, &when);
    let title = "Nightly (main)";
    let (published, _) = Cmd::new("gh").args(["release", "view", "nightly"]).attempt()?;
    if published {
        Cmd::new("gh")
            .args(["release", "edit", "nightly", "--title", title, "--notes", &notes, "--prerelease", "--latest=false"])
            .run()?;
        Cmd::new("gh").args(["release", "upload", "nightly"]).args(&uploads).arg("--clobber").run()?;
        // Anything left from an older snapshot that this one did not replace.
        let names =
            Cmd::new("gh").args(["release", "view", "nightly", "--json", "assets", "-q", ".assets[].name"]).stdout()?;
        let local: Vec<String> = uploads.iter().map(|p| name(p)).collect();
        for old in stale(&names, &local) {
            Cmd::new("gh").args(["release", "delete-asset", "nightly", old, "--yes"]).run()?;
        }
        Ok(())
    } else {
        Cmd::new("gh")
            .args(["release", "create", "nightly"])
            .args(&uploads)
            .args(["--verify-tag", "--title", title, "--notes", &notes, "--prerelease", "--latest=false"])
            .run()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installers_are_told_by_their_ends() {
        for file in [
            "Sidevoice_0.3.0_aarch64.dmg",
            "Sidevoice_0.3.0_x64-setup.exe",
            "Sidevoice_0.3.0_amd64.deb",
            "Sidevoice_0.3.0_amd64.AppImage",
        ] {
            assert!(is_installer(file), "{file}");
        }
        for file in ["SHA256SUMS.txt", "LEEME-primera-apertura.txt", "Sidevoice_0.3.0_x64.exe", "icon_16x16.png"] {
            assert!(!is_installer(file), "{file}");
        }
    }

    #[test]
    fn the_nightly_takes_fixed_names_and_a_release_keeps_its_own() {
        let dmg = "Sidevoice_0.3.0-rc.1_aarch64.dmg";
        assert_eq!(published_name(dmg, "0.3.0-rc.1", false).unwrap(), dmg);
        assert_eq!(published_name(dmg, "0.3.0-rc.1", true).unwrap(), "Sidevoice_nightly_aarch64.dmg");
        assert_eq!(
            published_name("Sidevoice_0.3.0_amd64.AppImage", "0.3.0", true).unwrap(),
            "Sidevoice_nightly_amd64.AppImage"
        );
        assert!(published_name("Sidevoice_0.2.9_amd64.deb", "0.3.0", true).is_err());
    }

    #[test]
    fn every_kind_must_be_there() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let all = names(&["a.dmg", "a_x64-setup.exe", "a.deb", "a.AppImage"]);
        assert!(missing_kinds(&all).is_empty());
        assert_eq!(missing_kinds(&names(&["a.dmg", "a.deb"])), ["exe", "AppImage"]);
    }

    #[test]
    fn a_release_candidate_is_never_latest() {
        assert_eq!(release_flags("v0.3.0-rc.1"), ["--draft=false", "--prerelease", "--latest=false"]);
        assert_eq!(release_flags("v0.3.0"), ["--draft=false", "--prerelease=false", "--latest"]);
    }

    #[test]
    fn the_nightly_drops_only_what_it_does_not_replace() {
        let local = vec!["Sidevoice_nightly_aarch64.dmg".to_owned(), "SHA256SUMS".to_owned()];
        assert_eq!(stale("Sidevoice_nightly_aarch64.dmg\nSHA256SUMS\nold.zip\n\n", &local), ["old.zip"]);
        assert!(nightly_notes("abc", "2026-10-12T10:00:00Z")
            .starts_with("Snapshot of `main` at abc (committed 2026-10-12T10:00:00Z)."));
    }

    #[test]
    fn sums_are_sha256sums_lines() {
        let dir = std::env::temp_dir().join(format!("xtask-sums-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.dmg");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(sums(&[file]).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a.dmg\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
