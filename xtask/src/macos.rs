//! The macOS app as built, checked and started: what `dist` and `smoke` share.

use crate::util::{fresh_dir, has, output, read, repo, run, sleep, target, wait_for, Background, Checks};
use crate::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const TRIPLE: &str = "aarch64-apple-darwin";
/// The app's identifier (tauri.conf.json): its directory under Application Support.
const IDENTIFIER: &str = "dev.sidevoice.desktop";
/// Where the stand-in node of test/fixtures/fake-node.py listens.
pub const FAKE_NODE_PORT: &str = "8768";

/// What only the `probe` build carries (src-tauri/src/probe.rs): never in a release app.
pub const PROBE_ONLY: &[&str] = &[
    "SIDEVOICE_DEBUG_PAGE",
    "probe-engine",
    "probe-card",
    "SIDEVOICE_DEBUG_ROOM_FLOW",
    "room-flow",
    "SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS",
    "SIDEVOICE_DEBUG_REFUSE_LOAD",
    "SIDEVOICE_DEBUG_SLOW_TRANSCRIBE",
    "SIDEVOICE_DEBUG_LOCAL_HOST_FLOW",
    "local-host-flow",
];

pub fn app() -> PathBuf {
    target().join(TRIPLE).join("release/bundle/macos/Sidevoice.app")
}

pub fn binary() -> PathBuf {
    app().join("Contents/MacOS/sidevoice-desktop")
}

/// Builds the .app (ad-hoc signed, hardened runtime, entitlements: tauri.conf.json), with `features` when given.
pub fn build(features: Option<&str>) -> Result<()> {
    let mut args = vec!["build", "--target", TRIPLE, "--bundles", "app"];
    if let Some(features) = features {
        args.extend(["--features", features]);
    }
    run(&mut crate::util::tauri(&args))
}

/// A scratch home for one run of the app (short: a Unix socket's path is at most 104 bytes): its settings, models
/// and pairing go there, never into this machine's.
pub fn scratch_home(name: &str) -> Result<PathBuf> {
    let home = PathBuf::from("/tmp").join(format!("svx-{name}"));
    fresh_dir(&home)?;
    Ok(home)
}

/// The app's config and data directory under `home`.
pub fn app_dir(home: &Path) -> PathBuf {
    home.join("Library/Application Support").join(IDENTIFIER)
}

/// The app's binary, with diagnostics on and `home` as its home.
pub fn app_command(home: &Path, env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(binary());
    command.env("HOME", home).env("SIDEVOICE_DEBUG", "1").envs(env.iter().copied());
    command
}

/// The stand-in node (test/fixtures/fake-node.py) on loopback.
pub fn fake_node(log: &Path) -> Result<Background> {
    let mut command = Command::new("python3");
    command.args(["-u", "test/fixtures/fake-node.py", FAKE_NODE_PORT]).current_dir(repo());
    Background::start(&mut command, log)
}

fn plist(app: &Path, key: &str) -> Result<String> {
    let info = app.join("Contents/Info.plist");
    Ok(output(Command::new("plutil").args(["-extract", key, "raw"]).arg(&info))?.trim().to_owned())
}

/// What a first open depends on, in the app as built.
pub fn verify_app(app: &Path, checks: &mut Checks) -> Result<()> {
    eprintln!("--- Info.plist");
    eprintln!("{}", output(Command::new("plutil").arg("-p").arg(app.join("Contents/Info.plist")))?);
    checks.check(plist(app, "NSMicrophoneUsageDescription").is_ok_and(|s| !s.is_empty()), "no microphone usage text");
    checks.check(plist(app, "LSMinimumSystemVersion")? == "13.0", "LSMinimumSystemVersion is not 13.0");
    checks.check(plist(app, "CFBundleName")? == "Sidevoice", "CFBundleName is not Sidevoice");

    eprintln!("--- signature");
    run(Command::new("codesign").args(["--verify", "--deep", "--strict", "--verbose=2"]).arg(app))?;
    let signature = output(Command::new("codesign").arg("-dvvv").arg(app))?;
    eprintln!("{signature}");
    checks.check(signature.contains("Signature=adhoc"), "the app is not ad-hoc signed");
    checks.check(signature.contains("runtime"), "the app is not signed with the hardened runtime");

    eprintln!("--- entitlements");
    let entitlements = entitlements(app)?;
    eprintln!("{entitlements}");
    for entitlement in ["com.apple.security.device.audio-input", "com.apple.security.cs.disable-library-validation"] {
        checks.check(entitlements.contains(entitlement), format!("the app lacks the entitlement {entitlement}"));
    }

    eprintln!("--- icon");
    checks.check(plist(app, "CFBundleIconFile")? == "icon.icns", "CFBundleIconFile is not icon.icns");
    let iconset = target().join("xtask/icon.iconset");
    let _ = fs::remove_dir_all(&iconset);
    fs::create_dir_all(iconset.parent().expect("has a parent")).map_err(|e| e.to_string())?;
    run(Command::new("iconutil")
        .args(["--convert", "iconset", "--output"])
        .arg(&iconset)
        .arg(app.join("Contents/Resources/icon.icns")))?;
    for size in ["16x16", "16x16@2x", "32x32", "32x32@2x", "128x128", "128x128@2x", "256x256", "256x256@2x"]
        .into_iter()
        .chain(["512x512", "512x512@2x"])
    {
        let file = iconset.join(format!("icon_{size}.png"));
        checks.check(fs::metadata(&file).is_ok_and(|m| m.len() > 0), format!("the icon has no {size}"));
    }
    let largest = output(Command::new("sips").args(["-g", "pixelWidth"]).arg(iconset.join("icon_512x512@2x.png")))?;
    checks.check(largest.contains("1024"), "the largest icon is not 1024 pixels wide");

    eprintln!("--- architecture");
    let binary = app.join("Contents/MacOS/sidevoice-desktop");
    let archs = output(Command::new("lipo").arg("-archs").arg(&binary))?;
    eprintln!("{archs}");
    checks.check(archs.contains("arm64"), "the binary is not arm64");

    eprintln!("--- no CI probe in a release (src-tauri/src/probe.rs is behind the probe feature)");
    let bytes = fs::read(&binary).map_err(|e| format!("{}: {e}", binary.display()))?;
    for marker in PROBE_ONLY {
        checks.check(!contains(&bytes, marker), format!("the release app carries {marker}"));
    }
    Ok(())
}

pub fn entitlements(app: &Path) -> Result<String> {
    output(Command::new("codesign").args(["-d", "--entitlements", "-", "--xml"]).arg(app))
}

pub fn contains(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|window| window == text.as_bytes())
}

/// First open, no settings: the bundled interface comes up in the real WKWebView, and asking for the probe page
/// changes nothing in a release.
pub fn first_open(checks: &mut Checks) -> Result<()> {
    let home = scratch_home("first-open")?;
    let log = target().join("xtask/first-open.log");
    let mut app = Background::start(
        &mut app_command(&home, &[("SIDEVOICE_DEBUG_PAGE", "probe.html"), ("SIDEVOICE_DEBUG_ROOM_FLOW", "1")]),
        &log,
    )?;
    wait_for(&log, "ready: true", 0, 45);
    sleep(5.0);
    let stayed = app.running();
    app.stop();
    let text = read(&log);
    eprintln!("{text}");
    checks.check(stayed, "the app exited on its own");
    // The interface loaded from the app itself, and its controller came up: the bridge attached to the real web UI's
    // store and actions, and reached the app over IPC.
    checks.check(has(&text, "page tauri://localhost/voice/index.html"), "the bundled interface did not load");
    checks.check(!has(&text, "probe.html") && !has(&text, "room-flow"), "a release ran a CI probe");
    checks.check(has(&text, "ready: true"), "the interface's controller never came up");
    Ok(())
}

/// With a target (a fake node on loopback): the interface asks it from the app's own origin, with no pairing yet.
pub fn with_target(checks: &mut Checks) -> Result<()> {
    let home = scratch_home("target")?;
    let conf = app_dir(&home);
    fs::create_dir_all(&conf).map_err(|e| e.to_string())?;
    let target_url = format!("http://127.0.0.1:{FAKE_NODE_PORT}");
    fs::write(conf.join("settings.json"), format!(r#"{{"target":"{target_url}","muteShortcut":""}}"#))
        .map_err(|e| e.to_string())?;
    let node_log = target().join("xtask/target-node.log");
    let app_log = target().join("xtask/target.log");
    let mut node = fake_node(&node_log)?;
    let mut app = Background::start(&mut app_command(&home, &[]), &app_log)?;
    wait_for(&app_log, "ready: true", 0, 45);
    sleep(5.0);
    app.stop();
    node.stop();
    let (app_text, node_text) = (read(&app_log), read(&node_log));
    eprintln!("--- app\n{app_text}\n--- fake node (what the interface asked, cross-origin)\n{node_text}");
    checks.check(has(&app_text, "ready: true"), "the interface's controller never came up with a target");
    checks.check(
        has(&node_text, "origin=tauri://localhost"),
        "the interface never asked its target from the app's origin",
    );
    Ok(())
}

/// The brand's .dmg (background, icon layout, volume icon: scripts/dmg-settings.py), checked as shipped.
pub fn dmg(app: &Path, out: &Path) -> Result<PathBuf> {
    let work = target().join("xtask/dmg");
    fresh_dir(&work)?;
    let venv = work.join("dmgbuild");
    run(Command::new("python3").args(["-m", "venv"]).arg(&venv))?;
    run(Command::new(venv.join("bin/pip")).args(["install", "--quiet", "dmgbuild==1.6.7"]))?;
    // One TIFF with the 1x and 2x drawings: Finder picks the one for the screen.
    let background = work.join("background.tiff");
    let art = repo().join("src-tauri/dmg");
    run(Command::new("tiffutil")
        .arg("-cathidpicheck")
        .arg(art.join("background.png"))
        .arg(art.join("background@2x.png"))
        .arg("-out")
        .arg(&background))?;
    let dmg = out.join(format!("Sidevoice_{}_aarch64.dmg", crate::util::version()?));
    let define = |key: &str, value: &Path| format!("{key}={}", value.display());
    run(Command::new(venv.join("bin/dmgbuild"))
        .arg("-s")
        .arg(repo().join("scripts/dmg-settings.py"))
        .args(["-D", &define("app", app), "-D", &define("background", &background)])
        .args(["-D", &define("layout", &art.join("layout.json"))])
        .args(["-D", &define("icon", &repo().join("src-tauri/icons/icon.icns"))])
        .arg("Sidevoice")
        .arg(&dmg))?;
    run(Command::new("hdiutil").arg("verify").arg(&dmg))?;

    // The app as shipped, inside the image, not the one in the build tree.
    let mount = work.join("mount");
    fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
    run(Command::new("hdiutil").args(["attach", "-readonly", "-nobrowse", "-mountpoint"]).arg(&mount).arg(&dmg))?;
    let mut checks = Checks::default();
    let checked = inside(&mount, &mut checks);
    run(Command::new("hdiutil").arg("detach").arg(&mount))?;
    checked?;
    checks.done("the .dmg")?;
    Ok(dmg)
}

fn inside(mount: &Path, checks: &mut Checks) -> Result<()> {
    eprintln!("{}", output(Command::new("ls").arg("-la").arg(mount))?);
    let nonempty = |name: &str| fs::metadata(mount.join(name)).is_ok_and(|m| m.len() > 0);
    for name in [".background.tiff", ".DS_Store", ".VolumeIcon.icns"] {
        checks.check(nonempty(name), format!("the .dmg has no {name}"));
    }
    let applications = fs::symlink_metadata(mount.join("Applications"));
    checks.check(applications.is_ok_and(|m| m.file_type().is_symlink()), "the .dmg has no Applications link");
    let app = mount.join("Sidevoice.app");
    run(Command::new("codesign").args(["--verify", "--deep", "--strict", "--verbose=2"]).arg(&app))?;
    checks.check(
        entitlements(&app)?.contains("com.apple.security.device.audio-input"),
        "the app in the .dmg lacks the microphone entitlement",
    );
    checks.check(plist(&app, "NSMicrophoneUsageDescription").is_ok(), "the app in the .dmg has no microphone text");
    Ok(())
}
