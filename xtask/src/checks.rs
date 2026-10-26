//! The checks of every pull request (the `test` job, Linux): the web interface at the pin, lint, the tests, the
//! icons, and the local host with two users.

use std::fs;
use std::path::Path;

use crate::util::{chmod, ensure, grep, mkdir, npm, read, repo, require_ci, require_os, sleep, Cmd};
use crate::Result;

/// `web`: the web interface at the pin (web.pin.json), into ui/voice/ (`npm run web`, scripts/build-web.mjs).
pub fn web() -> Result<()> {
    Cmd::new(npm()).args(["run", "web"]).run()
}

/// `lint`: rustfmt and Clippy, warnings denied, on the app's workspace (src-tauri/) and on this tooling. Both with
/// the app's toolchain (src-tauri/rust-toolchain.toml) and the app's rustfmt.toml.
pub fn lint() -> Result<()> {
    let app = repo().join("src-tauri");
    let xtask = repo().join("xtask/Cargo.toml");
    Cmd::new("cargo").args(["fmt", "--all", "--check"]).dir(&app).run()?;
    Cmd::new("cargo")
        .args(["fmt", "--manifest-path"])
        .arg(&xtask)
        .args(["--check", "--", "--config-path"])
        .arg(app.join("rustfmt.toml"))
        .dir(&app)
        .run()?;
    clippy(false)
}

/// The independent boundary job denies OS APIs; regular lint leaves those findings to that job.
pub fn clippy(boundary: bool) -> Result<()> {
    let app = repo().join("src-tauri");
    let xtask = repo().join("xtask/Cargo.toml");
    let level = if boundary { "-D" } else { "-A" };
    let flags = ["--", "-D", "warnings", level, "clippy::disallowed_methods", level, "clippy::disallowed_types"];
    // Run both even when the app reports boundary findings.
    let app_result = Cmd::new("cargo")
        .args(["clippy", "--workspace", "--all-targets", "--all-features"])
        .args(flags)
        .dir(&app)
        .run();
    let xtask_result = Cmd::new("cargo")
        .args(["clippy", "--locked", "--all-targets", "--manifest-path"])
        .arg(&xtask)
        .args(flags)
        .dir(&app)
        .run();
    let errors: Vec<_> = [app_result, xtask_result].into_iter().filter_map(Result::err).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// `test`: the bridge script's tests (`npm test`) and the app's Rust tests (`cargo test --workspace`).
pub fn test() -> Result<()> {
    Cmd::new(npm()).arg("test").run()?;
    Cmd::new("cargo").args(["test", "--workspace"]).dir(repo().join("src-tauri")).run()
}

/// Portable fixtures, also run by the existing macOS and Windows PR jobs.
pub fn test_support() -> Result<()> {
    Cmd::new("cargo")
        .args(["test", "--locked", "-p", "sidevoice-desktop-test-support"])
        .args(["--test", "portable", "--test", "credentials"])
        .dir(repo().join("src-tauri"))
        .run()
}

/// Everything `npm run icons` writes.
const DRAWN: &[&str] = &["src-tauri/packaging"];

/// `icons`: the icons, tray marks and .dmg window are what the brand files draw: `npm run icons`, then nothing it
/// writes differs from what is committed.
pub fn icons() -> Result<()> {
    Cmd::new(npm()).args(["run", "icons"]).run()?;
    Cmd::new("git").args(["status", "--porcelain", "--"]).args(DRAWN).run()?;
    Cmd::new("git")
        .args(["diff", "--quiet", "--exit-code", "--"])
        .args(DRAWN)
        .run()
        .map_err(|_| "the committed icons are not what the brand files draw: run `npm run icons` and commit".into())
}

/// The second user of `two-users`.
const OTHER: &str = "sidevoice-other";

/// `two-users` (Linux, CI only: it adds a user): the local host with two users. `local-host-ci serve` runs as this
/// user (a stand-in core on the socket it publishes, the app's side paired with it, the proxy), `attack` as another, who must get
/// nowhere: it cannot open that socket, link as a connector, or use the proxy
/// (test-support/src/bin/local-host-ci.rs).
pub fn two_users() -> Result<()> {
    require_os("linux", "two-users")?;
    require_ci("two-users")?;
    let app = repo().join("src-tauri");
    Cmd::new("cargo")
        .args(["build", "-p", "sidevoice-desktop-test-support", "--bin", "local-host-ci"])
        .dir(&app)
        .run()?;
    // Outside any home, where the second user can reach it.
    let dir = Path::new("/tmp/svci");
    mkdir(dir)?;
    chmod(dir, 0o755)?;
    let tool = dir.join("local-host-ci");
    fs::copy(app.join("target/debug/local-host-ci"), &tool).map_err(|e| format!("{}: {e}", tool.display()))?;
    chmod(&tool, 0o755)?;
    if !Cmd::new("id").arg(OTHER).attempt()?.0 {
        Cmd::new("sudo").args(["useradd", "--create-home", OTHER]).run()?;
    }
    let (data, app_dir, port_file, log) = (dir.join("d"), dir.join("app"), dir.join("port"), dir.join("serve.log"));
    let _ = fs::remove_file(&port_file);
    let mut serve = Cmd::new(&tool).arg("serve").arg(&data).arg(&app_dir).arg(&port_file).spawn(&log)?;
    for _ in 0..50 {
        if fs::metadata(&port_file).is_ok_and(|m| m.len() > 0) {
            break;
        }
        sleep(0.2);
    }
    let port = fs::read_to_string(&port_file).map_err(|e| format!("serve wrote no port: {e}"))?.trim().to_owned();
    // Where the stand-in core listens: the endpoint it publishes, which the second user cannot read.
    let record =
        fs::read_to_string(data.join("core/core.json")).map_err(|e| format!("serve published no core record: {e}"))?;
    let socket = serde_json::from_str::<serde_json::Value>(&record)
        .ok()
        .and_then(|v| v["endpoint"].as_str().map(str::to_owned))
        .ok_or("core.json names no endpoint")?;
    Cmd::new("ls").arg("-la").arg(dir).arg(&data).run()?;
    eprintln!("--- the second user");
    Cmd::new("sudo")
        .args(["-u", OTHER])
        .arg(&tool)
        .arg("attack")
        .arg(&data)
        .arg(&app_dir)
        .arg(&port)
        .arg(&socket)
        .run()?;
    eprintln!("--- the same attempts as the first user (control: the socket and the files are open to it)");
    let (control, _) = Cmd::new(&tool).arg("attack").arg(&data).arg(&app_dir).arg(&port).arg(&socket).attempt()?;
    ensure(!control, "the control got nowhere")?;
    serve.stop();
    let text = read(&log);
    eprintln!("--- serve\n{text}");
    // The second user's attempts never reached the core: it saw only the first user's requests.
    ensure(grep(&text, "auth=other").is_empty(), "the core saw a foreign token")
}

/// `engine-rev`: the sidevoice-engine commit src-tauri/Cargo.lock pins, printed (CI keys the cache of sherpa-onnx's
/// libraries on it).
pub fn engine_rev() -> Result<()> {
    println!("{}", pinned_engine()?);
    Ok(())
}

fn pinned_engine() -> Result<String> {
    let lock = repo().join("src-tauri/Cargo.lock");
    engine_in_lock(&fs::read_to_string(&lock).map_err(|e| format!("{}: {e}", lock.display()))?)
}

/// The commit of the first `git+https://github.com/sidevoice/sidevoice-engine…#<commit>` source in a Cargo.lock.
pub fn engine_in_lock(lock: &str) -> Result<String> {
    let source = crate::util::re(r#""git\+https://github\.com/sidevoice/sidevoice-engine[^"]*""#)
        .find(lock)
        .map(|m| m.as_str().trim_matches('"'))
        .unwrap_or_default();
    let rev = source.rsplit('#').next().unwrap_or_default();
    ensure(rev.len() == 40, format!("no sidevoice-engine commit in src-tauri/Cargo.lock: {source:?}"))?;
    Ok(rev.to_owned())
}

/// `sherpa-libs`: sherpa-onnx's static libraries, which sidevoice-engine links, fetched by that engine's own `cargo
/// xtask sherpa-libs` at the commit src-tauri/Cargo.lock pins: checked against the digests it pins, kept outside
/// target/ (`~/.cache/sidevoice-engine/sherpa-onnx`), the directory printed and, on a runner, exported as
/// `SHERPA_ONNX_LIB_DIR` (docs/ENGINES.md, "Building"). So no build script downloads anything unchecked.
pub fn sherpa_libs() -> Result<()> {
    let rev = pinned_engine()?;
    let base = std::env::var_os("RUNNER_TEMP").map_or_else(|| repo().join("target/xtask"), Into::into);
    mkdir(&base)?;
    let engine = base.join("sidevoice-engine");
    if engine.exists() {
        fs::remove_dir_all(&engine).map_err(|e| format!("{}: {e}", engine.display()))?;
    }
    let quiet = |cmd: Cmd| cmd.stdout().map(drop);
    quiet(Cmd::new("git").args(["init", "-q"]).arg(&engine))?;
    quiet(Cmd::new("git").arg("-C").arg(&engine).args([
        "fetch",
        "-q",
        "--depth",
        "1",
        "https://github.com/sidevoice/sidevoice-engine",
        &rev,
    ]))?;
    quiet(Cmd::new("git").arg("-C").arg(&engine).args(["checkout", "-q", "FETCH_HEAD"]))?;
    // From src-tauri, so the app's toolchain builds the engine's xtask too. Without its default features it does not
    // build the engine (nor sherpa-onnx-sys) before the libraries are there. Its standard output (the directory) is
    // this command's.
    Cmd::new("cargo")
        .args(["run", "--quiet", "--locked", "--manifest-path"])
        .arg(engine.join("xtask/Cargo.toml"))
        .args(["--no-default-features", "--", "sherpa-libs"])
        .dir(repo().join("src-tauri"))
        .run()
}

/// `bundle [--channel release|nightly|dev]`: this host's installers, as a release ships them: the .deb and the
/// AppImage on Linux, the NSIS installer on Windows (macOS: `macos app`, `macos dmg`), built for the channel (default
/// `dev`). A release's or a nightly's also signs the AppImage and the installer as updates (a `.deb` cannot update
/// itself; its `.sig` is not published); `publish assets` checks the signatures.
pub fn bundle(channel: crate::util::Channel) -> Result<()> {
    let bundles = match std::env::consts::OS {
        "linux" => "deb,appimage",
        "windows" => "nsis",
        os => return Err(format!("`cargo xtask bundle` is for Linux and Windows, not {os}: see `cargo xtask macos`")),
    };
    let mut command = crate::util::TauriBuild { bundles, channel, ..Default::default() }.command()?;
    if std::env::consts::OS == "windows" {
        // Installers must run on other CPUs too, including when bundled on a laptop.
        command = command.env("GGML_NATIVE", "OFF");
    }
    command.run()
}

/// `release-pr-title [BRANCH]`: the title of the release PR on BRANCH (default `GITHUB_REF_NAME`, the branch a
/// dispatched pr-title run is on), which must be release-please's: "chore(main): release X.Y.Z". `gh` with `GH_TOKEN`
/// and `GH_REPO` from the environment.
pub fn release_pr_title(branch: Option<&str>) -> Result<()> {
    let branch = match branch {
        Some(branch) => branch.to_owned(),
        None => std::env::var("GITHUB_REF_NAME").map_err(|_| "no BRANCH, and GITHUB_REF_NAME is not set")?,
    };
    ensure(is_release_branch(&branch), "dispatch is for release-please's branch only")?;
    let title = Cmd::new("gh").args(["pr", "view", &branch, "--json", "title", "-q", ".title"]).stdout()?;
    let title = title.trim_end_matches('\n');
    eprintln!("{title}");
    ensure(is_release_title(title), format!("not a release PR's title: {title}"))
}

pub fn is_release_branch(branch: &str) -> bool {
    branch.starts_with("release-please--")
}

pub fn is_release_title(title: &str) -> bool {
    title.lines().any(|line| crate::util::re(r"^chore(\([^)]+\))?: release ").is_match(line))
}

/// `release-pr-checks`: the PR-title check dispatched on every open release PR's branch (release-please opens and
/// updates its PR with the workflow's token, whose pushes start no workflow). `gh` with `GH_TOKEN` and `GH_REPO`.
pub fn release_pr_checks() -> Result<()> {
    let branches = Cmd::new("gh")
        .args(["pr", "list", "--state", "open", "--label", "autorelease: pending"])
        .args(["--json", "headRefName", "--jq", ".[].headRefName"])
        .stdout()?;
    for branch in branches.split_whitespace() {
        Cmd::new("gh").args(["workflow", "run", "pr-title.yml", "--ref", branch]).run()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_commit_is_read_from_the_lock() {
        let lock = r#"
[[package]]
name = "sidevoice-engine"
version = "0.4.0"
source = "git+https://github.com/sidevoice/sidevoice-engine?rev=0123456#0123456789abcdef0123456789abcdef01234567"
"#;
        assert_eq!(engine_in_lock(lock).unwrap(), "0123456789abcdef0123456789abcdef01234567");
        assert!(engine_in_lock("source = \"git+https://github.com/sidevoice/sidevoice-engine#abc\"").is_err());
        assert!(engine_in_lock("nothing").is_err());
    }

    #[test]
    fn only_release_pleases_branch_and_title_pass() {
        assert!(is_release_branch("release-please--branches--main"));
        assert!(!is_release_branch("main"));
        assert!(is_release_title("chore(main): release 0.2.0"));
        assert!(is_release_title("chore: release 0.2.0"));
        assert!(!is_release_title("feat: release notes"));
        assert!(!is_release_title("chore(main): released"));
    }
}
