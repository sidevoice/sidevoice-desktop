//! `publish …`: a run's installers onto GitHub (RELEASING.md): the release that called the build, or the rolling
//! `nightly` pre-release. `gh`, with `GH_TOKEN` and `GH_REPO` from the environment.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::util::{ci_nightly_version, ensure, files, name, repo, version, write, Cmd};
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

/// The name an installer or update is published under: as built for a release; for the nightly, `_nightly_` where its
/// version was (`Sidevoice_0.3.0_aarch64.dmg`, or `Sidevoice_0.3.1-nightly.5_amd64.AppImage` as the bundler names a
/// nightly's → `Sidevoice_nightly_aarch64.dmg`, …), so a link to it keeps working.
pub fn published_name(file: &str, nightly: bool) -> Result<String> {
    if !nightly {
        return Ok(file.to_owned());
    }
    match file.split_once('_').and_then(|(product, rest)| Some((product, rest.split_once('_')?))) {
        Some((product, (version, rest))) if version.starts_with(|c: char| c.is_ascii_digit()) => {
            Ok(format!("{product}_nightly_{rest}"))
        }
        _ => Err(format!("{file} does not carry a version")),
    }
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

/// What a release adds for the updater (docs/BRIDGE.md → "The updater"), by how the names end: each platform's update
/// (macOS's app as a .tar.gz; Linux's and Windows' are the AppImage and the installer themselves) and the signatures.
/// Never on the nightly: only a release is offered as an update.
const UPDATES: &[&str] = &[".app.tar.gz", ".app.tar.gz.sig", ".AppImage.sig", "-setup.exe.sig"];

pub fn is_update(file: &str) -> bool {
    UPDATES.iter().any(|end| file.ends_with(end))
}

/// Each platform a release updates (latest.json's key), and its update's name for `version`.
pub fn update_files(version: &str) -> [(&'static str, String); 3] {
    [
        ("darwin-aarch64", format!("Sidevoice_{version}_aarch64.app.tar.gz")),
        ("windows-x86_64", format!("Sidevoice_{version}_x64-setup.exe")),
        ("linux-x86_64", format!("Sidevoice_{version}_amd64.AppImage")),
    ]
}

/// latest.json (tauri-plugin-updater's static manifest): the version, the release's notes, when it was published, and
/// each platform's `{signature, url}`, the signature being the `.sig` file's text and the url the asset's on the
/// release (`base`: `https://github.com/<repo>/releases/download/<tag>`).
pub fn latest_json(version: &str, notes: &str, pub_date: &str, base: &str, sigs: &[(&str, String, String)]) -> String {
    let platforms: serde_json::Map<String, serde_json::Value> = sigs
        .iter()
        .map(|(platform, file, signature)| {
            (platform.to_string(), serde_json::json!({ "signature": signature, "url": format!("{base}/{file}") }))
        })
        .collect();
    let manifest =
        serde_json::json!({ "version": version, "notes": notes, "pub_date": pub_date, "platforms": platforms });
    serde_json::to_string_pretty(&manifest).expect("JSON serialises") + "\n"
}

/// `publish assets release TAG | publish assets nightly`: the installers among the build's artifacts (`artifacts/`,
/// as actions/download-artifact leaves them) into `assets/`, under their published names, each kind there; also every
/// platform's update and its signature, checked, and latest.json (a release's notes are release TAG's, `gh` with
/// `GH_REPO`; the nightly's version is [`ci_nightly_version`]); and their `SHA256SUMS`.
pub fn assemble(kind: &str, tag: Option<&str>) -> Result<()> {
    let (tag, nightly) = match (kind, tag) {
        ("release", Some(tag)) => (tag, false),
        ("nightly", None) => ("nightly", true),
        _ => return Err("publish assets release TAG | publish assets nightly".into()),
    };
    let version = version()?;
    let assets = assets();
    // As `mkdir assets` did: never over an earlier one.
    ensure(!assets.exists(), format!("{} is there already: remove it first", assets.display()))?;
    crate::util::mkdir(&assets)?;
    for file in all_files(&artifacts())? {
        let built = name(&file);
        if is_installer(&built) || is_update(&built) {
            let to = assets.join(published_name(&built, nightly)?);
            fs::copy(&file, &to).map_err(|e| format!("{}: {e}", file.display()))?;
        }
    }
    let names: Vec<String> = files(&assets)?.iter().map(|p| name(p)).collect();
    let missing = missing_kinds(&names);
    ensure(missing.is_empty(), format!("no {} among the artifacts", missing.join(", ")))?;
    let mut sigs = Vec::new();
    for (platform, file) in update_files(if nightly { "nightly" } else { &version }) {
        let signature = fs::read_to_string(assets.join(format!("{file}.sig"))).unwrap_or_default();
        ensure(!signature.trim().is_empty(), format!("no {file}.sig among the artifacts, or it is empty"))?;
        ensure(fs::metadata(assets.join(&file)).is_ok_and(|m| m.len() > 0), format!("no {file}, or it is empty"))?;
        sigs.push((platform, file, signature));
    }
    let (version, notes) = if nightly {
        (ci_nightly_version()?, "A nightly build of `main`, not a versioned release.".to_owned())
    } else {
        (version, Cmd::new("gh").args(["release", "view", tag, "--json", "body", "-q", ".body"]).stdout()?)
    };
    let repo_name = std::env::var("GH_REPO").map_err(|_| "GH_REPO is not set (owner/name)")?;
    let now = Cmd::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"]).stdout()?;
    let base = format!("https://github.com/{repo_name}/releases/download/{tag}");
    let latest = latest_json(&version, notes.trim_end(), now.trim(), &base, &sigs);
    write(&assets.join("latest.json"), &latest)?;
    eprint!("{latest}");
    let present: Vec<PathBuf> = files(&assets)?;
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
        assert_eq!(published_name(dmg, false).unwrap(), dmg);
        assert_eq!(published_name(dmg, true).unwrap(), "Sidevoice_nightly_aarch64.dmg");
        // As the bundler names a nightly's (built as its own version), and the updates.
        for (built, published) in [
            ("Sidevoice_0.3.1-nightly.5_amd64.AppImage", "Sidevoice_nightly_amd64.AppImage"),
            ("Sidevoice_0.3.1-nightly.5_x64-setup.exe.sig", "Sidevoice_nightly_x64-setup.exe.sig"),
            ("Sidevoice_0.3.0_aarch64.app.tar.gz", "Sidevoice_nightly_aarch64.app.tar.gz"),
        ] {
            assert_eq!(published_name(built, true).unwrap(), published);
        }
        assert!(published_name("Sidevoice_amd64.deb", true).is_err());
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
    fn updates_are_told_by_their_ends_and_are_not_installers() {
        for file in [
            "Sidevoice_0.4.0_aarch64.app.tar.gz",
            "Sidevoice_0.4.0_aarch64.app.tar.gz.sig",
            "Sidevoice_0.4.0_amd64.AppImage.sig",
            "Sidevoice_0.4.0_x64-setup.exe.sig",
        ] {
            assert!(is_update(file), "{file}");
            assert!(!is_installer(file), "{file}");
        }
        for file in ["Sidevoice_0.4.0_amd64.deb.sig", "Sidevoice_0.4.0_amd64.AppImage", "SHA256SUMS.txt", "latest.json"]
        {
            assert!(!is_update(file), "{file}");
        }
    }

    #[test]
    fn latest_json_is_the_updater_s_static_manifest() {
        let sigs: Vec<(&str, String, String)> = update_files("0.4.0")
            .into_iter()
            .map(|(platform, file)| (platform, file.clone(), format!("sig of {file}")))
            .collect();
        let base = "https://github.com/sidevoice/sidevoice-desktop/releases/download/v0.4.0";
        let text = latest_json("0.4.0", "## 0.4.0\n- notes", "2026-10-13T08:00:00Z", base, &sigs);
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            manifest,
            serde_json::json!({
                "version": "0.4.0",
                "notes": "## 0.4.0\n- notes",
                "pub_date": "2026-10-13T08:00:00Z",
                "platforms": {
                    "darwin-aarch64": {"signature": "sig of Sidevoice_0.4.0_aarch64.app.tar.gz", "url": format!("{base}/Sidevoice_0.4.0_aarch64.app.tar.gz")},
                    "windows-x86_64": {"signature": "sig of Sidevoice_0.4.0_x64-setup.exe", "url": format!("{base}/Sidevoice_0.4.0_x64-setup.exe")},
                    "linux-x86_64": {"signature": "sig of Sidevoice_0.4.0_amd64.AppImage", "url": format!("{base}/Sidevoice_0.4.0_amd64.AppImage")},
                }
            })
        );
    }

    #[test]
    fn the_nightly_s_latest_json_points_at_the_nightly_s_own_assets() {
        // What the build makes for a nightly, published under the names latest.json gives.
        let built = [
            "Sidevoice_0.4.2_aarch64.app.tar.gz",
            "Sidevoice_0.4.3-nightly.17_x64-setup.exe",
            "Sidevoice_0.4.3-nightly.17_amd64.AppImage",
        ];
        let files: Vec<String> = update_files("nightly").into_iter().map(|(_, file)| file).collect();
        let published: Vec<String> = built.iter().map(|b| published_name(b, true).unwrap()).collect();
        assert_eq!(published, files);
        let sigs: Vec<(&str, String, String)> =
            update_files("nightly").into_iter().map(|(platform, file)| (platform, file, "sig".to_owned())).collect();
        let base = "https://github.com/sidevoice/sidevoice-desktop/releases/download/nightly";
        let manifest: serde_json::Value =
            serde_json::from_str(&latest_json("0.4.3-nightly.17", "", "2026-10-13T08:00:00Z", base, &sigs)).unwrap();
        assert_eq!(manifest["version"], "0.4.3-nightly.17");
        assert_eq!(
            manifest["platforms"]["linux-x86_64"]["url"],
            "https://github.com/sidevoice/sidevoice-desktop/releases/download/nightly/Sidevoice_nightly_amd64.AppImage"
        );
        assert_eq!(
            manifest["platforms"]["darwin-aarch64"]["url"],
            format!("{base}/Sidevoice_nightly_aarch64.app.tar.gz")
        );
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
