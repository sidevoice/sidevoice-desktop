//! `publish DIR TAG`: put what `manifest` prepared on the GitHub release TAG, read it back, and publish it
//! (RELEASING.md). Runs `gh` with `GH_TOKEN` and `GH_REPO` from the environment.
//!
//! - `vX.Y.Z` (the draft release-please created): attach, check, publish; a version with a `-` suffix is a pre-release
//!   and never latest.
//! - `nightly`: move the tag to this commit, replace every asset of the one `nightly` pre-release (creating it the
//!   first time), drop what an older snapshot left.

use crate::manifest::{files, name, sha256, SUMS};
use crate::util::{fresh_dir, output, repo, run, target};
use crate::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

fn gh(args: &[&str]) -> Command {
    let mut command = Command::new("gh");
    command.args(args).current_dir(repo());
    command
}

pub fn publish(dir: &Path, tag: &str) -> Result<()> {
    let assets: Vec<String> = files(dir)?.iter().map(|p| p.display().to_string()).collect();
    if !assets.iter().any(|a| a.ends_with(SUMS)) {
        return Err(format!("{} has no {SUMS}: run `cargo xtask manifest` first", dir.display()));
    }
    let attach = |command: &mut Command| run(command.args(&assets).arg("--clobber"));
    if tag == "nightly" {
        nightly(&assets)?;
    } else {
        attach(&mut gh(&["release", "upload", tag]))?;
    }
    read_back(dir, tag)?;
    if tag != "nightly" {
        let edit: &[&str] = if tag.contains('-') {
            &["--draft=false", "--prerelease", "--latest=false"]
        } else {
            &["--draft=false", "--prerelease=false", "--latest"]
        };
        run(gh(&["release", "edit", tag]).args(edit))?;
    }
    Ok(())
}

fn nightly(assets: &[String]) -> Result<()> {
    let sha = match std::env::var("GITHUB_SHA") {
        Ok(sha) if !sha.is_empty() => sha,
        _ => output(Command::new("git").arg("-C").arg(repo()).args(["rev-parse", "HEAD"]))?.trim().to_owned(),
    };
    let when = output(Command::new("git").arg("-C").arg(repo()).args(["show", "-s", "--format=%cI", &sha]))?;
    let repo_name = std::env::var("GH_REPO").map_err(|_| "GH_REPO names no repository")?;
    let tag_ref = format!("repos/{repo_name}/git/ref/tags/nightly");
    if gh(&["api", &tag_ref]).output().is_ok_and(|o| o.status.success()) {
        let refs = format!("repos/{repo_name}/git/refs/tags/nightly");
        run(&mut gh(&["api", "-X", "PATCH", &refs, "-f", &format!("sha={sha}"), "-F", "force=true", "--silent"]))?;
    } else {
        let refs = format!("repos/{repo_name}/git/refs");
        run(&mut gh(&[
            "api",
            "-X",
            "POST",
            &refs,
            "-f",
            "ref=refs/tags/nightly",
            "-f",
            &format!("sha={sha}"),
            "--silent",
        ]))?;
    }
    let notes = format!(
        "Snapshot of `main` at {sha} (committed {}). Not a versioned release: the `nightly` tag moves to every commit \
         on `main` whose CI passes, and these assets are replaced each time. Versioned releases are the `vX.Y.Z` ones.",
        when.trim()
    );
    let title = "Nightly (main)";
    if gh(&["release", "view", "nightly"]).output().is_ok_and(|o| o.status.success()) {
        run(&mut gh(&[
            "release",
            "edit",
            "nightly",
            "--title",
            title,
            "--notes",
            &notes,
            "--prerelease",
            "--latest=false",
        ]))?;
        run(gh(&["release", "upload", "nightly"]).args(assets).arg("--clobber"))?;
        // Anything left from an older snapshot that this one did not replace.
        let published = output(&mut gh(&["release", "view", "nightly", "--json", "assets", "-q", ".assets[].name"]))?;
        for stale in
            published.lines().filter(|n| !n.is_empty() && !assets.iter().any(|a| a.ends_with(&format!("/{n}"))))
        {
            run(&mut gh(&["release", "delete-asset", "nightly", stale, "--yes"]))?;
        }
    } else {
        run(gh(&["release", "create", "nightly"]).args(assets).args([
            "--verify-tag",
            "--title",
            title,
            "--notes",
            &notes,
            "--prerelease",
            "--latest=false",
        ]))?;
    }
    Ok(())
}

/// Every asset as the release now serves it, checked against `SHA256SUMS`.
fn read_back(dir: &Path, tag: &str) -> Result<()> {
    let back = target().join("xtask/read-back");
    fresh_dir(&back)?;
    run(gh(&["release", "download", tag, "--dir"]).arg(&back))?;
    let sums = fs::read_to_string(dir.join(SUMS)).map_err(|e| e.to_string())?;
    for line in sums.lines() {
        let (digest, file) = line.split_once("  ").ok_or_else(|| format!("{SUMS}: {line}"))?;
        let served = sha256(&back.join(file))?;
        if served != digest {
            return Err(format!("{tag} serves {file} with sha256 {served}, not {digest}"));
        }
    }
    let served: Vec<String> = files(&back)?.iter().map(|p| name(p)).collect();
    eprintln!("{tag}: {} assets read back and checked: {served:?}", sums.lines().count());
    Ok(())
}
