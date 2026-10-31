//! The macOS app and its .dmg, built, signed, checked and started as a release ships them (RELEASING.md, "macOS signing
//! and notarization"). The probe build and the native flows are in probe.rs.

use sidevoice_platform::fs::{Create, Files, Fs};
use std::fs;
use std::path::{Path, PathBuf};

use crate::platform::macos_app::{
    allow_codesign, app, binary, create_keychain, import_certificate, keychain_list, notarized, notary_status, region,
    release_target, signing_keychain, unlock_keychain, unsigned, Signature, KEYCHAIN, PROBE_ONLY, SIGNING,
};
use crate::platform::private_files::{PrivateCreation, PrivateFiles};
use crate::util::{
    contains, copy_dir, ensure, fresh_dir, in_ci, mkdir, random_hex, read, read_bytes, read_secret, repo, required_env,
    secret_path, sleep, version, work, Channel, Cmd, SecretDir, TauriBuild,
};
use crate::Result;

/// The notary's API key (App Store Connect) as the file `notarytool` and Tauri's bundler read: `APPLE_API_KEY_P8`
/// (its contents) written to a file only this user may read, removed with `dir`; or, on a laptop, the file
/// `APPLE_API_KEY_PATH` names, read through a checked private handle and staged in the same private directory.
fn notary_key(dir: &SecretDir) -> Result<PathBuf> {
    match std::env::var("APPLE_API_KEY_P8") {
        Ok(p8) if !p8.is_empty() => dir.file("AuthKey.p8", format!("{p8}\n").as_bytes()),
        _ => match std::env::var_os("APPLE_API_KEY_PATH") {
            Some(path) => {
                let bytes = read_secret(Path::new(&path))?;
                dir.file("AuthKey.p8", &bytes)
            }
            _ => Err("neither APPLE_API_KEY_P8 nor APPLE_API_KEY_PATH (a file) is set (RELEASING.md)".into()),
        },
    }
}

/// The .dmg `release package` makes: `dist/Sidevoice_<version>_aarch64.dmg`.
fn dmg_path() -> Result<PathBuf> {
    Ok(repo().join("dist").join(format!("Sidevoice_{}_aarch64.dmg", version()?)))
}

/// `release app [--signed] [--channel release|nightly|dev]`: the app bundle (hardened runtime,
/// desktop/packaging/macos/Entitlements.plist), built for its channel (default `dev`). Ad-hoc signed; or, with `--signed`, signed by
/// Tauri's bundler with the Developer ID (`APPLE_SIGNING_IDENTITY`; `APPLE_CERTIFICATE` and
/// `APPLE_CERTIFICATE_PASSWORD`, imported into a keychain of the bundler's own, or else the login keychain's),
/// notarized with the API key (`APPLE_API_KEY`, `APPLE_API_ISSUER`, and the key file, see [`notary_key`]) and
/// stapled. A release's (signed only) or a nightly's also makes the update artifact ([`update_artifact`]).
pub fn build_app(signed: bool, channel: Channel) -> Result<()> {
    let build = TauriBuild { target: Some(release_target().triple), bundles: "app", channel, ..TauriBuild::default() };
    ensure(
        signed || channel != Channel::Release,
        "a release's app is signed: `release app --signed --channel release`",
    )?;
    if signed {
        build_signed(build)?;
        staple_app()?;
    } else {
        unsigned(build.command()?).run()?;
    }
    if channel != Channel::Dev {
        update_artifact(signed)?;
    }
    Ok(())
}

fn build_signed(build: TauriBuild) -> Result<()> {
    for name in ["APPLE_SIGNING_IDENTITY", "APPLE_API_KEY", "APPLE_API_ISSUER"] {
        required_env(name)?;
    }
    forget_secrets()?;
    let _private = PrivateFiles::start();
    let secrets = SecretDir::new(NOTARY)?;
    let key = notary_key(&secrets)?;
    // The bundler imports the certificate into a keychain of its own, which it adds to the search list and removes
    // when done; one it left (a build that failed half-way) is removed here.
    let before = user_keychains()?;
    let built = build.command().and_then(|cmd| cmd.env("APPLE_API_KEY_PATH", &key).run());
    for left in user_keychains()?.into_iter().filter(|k| !before.contains(k)) {
        let _ = Cmd::new("security").arg("delete-keychain").arg(&left).attempt();
    }
    built
}

/// The update a running app downloads (docs/BRIDGE.md → "The updater"): the app as `dist/Sidevoice_<version>_aarch64.app.tar.gz`,
/// with its `.sig` (made with the updater's key, checked by the app against tauri.conf.json's public key). A `signed`
/// one is packed again from the app as it is once [`staple_app`] has stapled it (the bundler's own archive was made
/// before, and Apple's ticket may not have reached its CDN when the bundler stapled), signed again with the updater's
/// key, and checked unpacked as the app is: Developer ID signed, its ticket stapled, and Gatekeeper takes it as
/// notarized (a nightly's too).
fn update_artifact(signed: bool) -> Result<()> {
    let bundle = app().with_file_name("Sidevoice.app.tar.gz");
    let dist = repo().join("dist");
    mkdir(&dist)?;
    let update = dist.join(format!("Sidevoice_{}_aarch64.app.tar.gz", version()?));
    let sig = PathBuf::from(format!("{}.sig", update.display()));
    if signed {
        let app = app();
        let parent = app.parent().ok_or("the app has no parent directory")?;
        // COPYFILE_DISABLE: no AppleDouble (`._*`) entries beside the files in the archive.
        Cmd::new("tar")
            .env("COPYFILE_DISABLE", "1")
            .arg("-czf")
            .arg(&update)
            .arg("-C")
            .arg(parent)
            .arg("Sidevoice.app")
            .run()?;
        // The updater's key and its password are in the environment, as the bundler reads them.
        Cmd::new("node")
            .arg(repo().join("node_modules/@tauri-apps/cli/tauri.js"))
            .args(["signer", "sign"])
            .arg(&update)
            .run()?;
        ensure(sig.is_file(), format!("{} is not there: the updater's key signed nothing", sig.display()))?;
        // What it unpacks to is the notarized app, its ticket stapled.
        let unpacked = work("update-artifact")?.join("unpacked");
        fresh_dir(&unpacked)?;
        Cmd::new("tar").arg("-xzf").arg(&update).arg("-C").arg(&unpacked).run()?;
        stapler("validate", unpacked.join("Sidevoice.app"))?;
    } else {
        fs::copy(&bundle, &update).map_err(|e| format!("{}: {e}", bundle.display()))?;
        let bundle_sig = PathBuf::from(format!("{}.sig", bundle.display()));
        fs::copy(&bundle_sig, &sig).map_err(|e| format!("{}: {e}", bundle_sig.display()))?;
    }
    Ok(())
}

/// The built app's notarization ticket stapled, for minutes if need be. The bundler staples right after Apple accepts
/// the submission, and when the ticket has not reached the ticket service yet its stapling does not take, though the
/// build goes on; validating later cannot fix that, stapling again can.
fn staple_app() -> Result<()> {
    let app = app();
    stapler_waiting("staple", &app, &STAPLE_WAITS)?;
    stapler("validate", &app)
}

/// The user's keychain search list (`security list-keychains -d user`).
fn user_keychains() -> Result<Vec<String>> {
    Ok(keychain_list(&Cmd::new("security").args(["list-keychains", "-d", "user"]).stdout()?))
}

fn plist(app: &Path, key: &str) -> Result<String> {
    let info = app.join("Contents/Info.plist");
    Ok(Cmd::new("plutil").args(["-extract", key, "raw"]).arg(info).stdout()?.trim_end_matches('\n').to_owned())
}

pub fn entitlements(app: &Path) -> Result<String> {
    Cmd::new("codesign").args(["-d", "--entitlements", "-", "--xml"]).arg(app).stdout()
}

/// `release check-app [--signed]`: what a first open depends on, in the app as built: Info.plist, the signature (with
/// `--signed`: the Developer ID's, `APPLE_TEAM_ID`'s, timestamped, notarized and stapled, and Gatekeeper takes it),
/// the entitlements, the icon (its entries also copied to dmg-screens/ for the record), arm64, and no CI probe.
pub fn check_app(signed: bool) -> Result<()> {
    let app = app();
    ensure(app.is_dir(), format!("{} is not there: `cargo xtask release app` first", app.display()))?;
    let team = if signed { Some(required_env("APPLE_TEAM_ID")?) } else { None };
    eprintln!("--- Info.plist");
    Cmd::new("plutil").arg("-p").arg(app.join("Contents/Info.plist")).run()?;
    eprintln!("{}", plist(&app, "NSMicrophoneUsageDescription")?);
    ensure(plist(&app, "LSMinimumSystemVersion")? == "13.0", "LSMinimumSystemVersion is not 13.0")?;

    eprintln!("--- signature");
    Cmd::new("codesign").args(["--verify", "--deep", "--strict", "--verbose=2"]).arg(&app).run()?;
    let signature = Cmd::new("codesign").arg("-dvvv").arg(&app).output()?;
    eprintln!("{signature}");
    Signature::check(&signature, team.as_deref())?;
    if signed {
        stapler("validate", &app)?;
        let spctl = Cmd::new("spctl").args(["-a", "-vv", "-t", "exec"]).arg(&app).output()?;
        eprintln!("{spctl}");
        ensure(notarized(&spctl), "Gatekeeper does not take the app as a notarized Developer ID app")?;
    }

    eprintln!("--- entitlements");
    let entitlements = entitlements(&app)?;
    eprintln!("{entitlements}");
    ensure(entitlements.contains("com.apple.security.device.audio-input"), "no microphone entitlement")?;
    // The engine is linked into the app: no downloaded library is loaded, so library validation stays on.
    ensure(!entitlements.contains("com.apple.security.cs.disable-library-validation"), "library validation is off")?;

    eprintln!("--- icon");
    ensure(plist(&app, "CFBundleIconFile")? == "icon.icns", "CFBundleIconFile is not icon.icns")?;
    let iconset = work("check-app")?.join("icon.iconset");
    if iconset.exists() {
        fs::remove_dir_all(&iconset).map_err(|e| format!("{}: {e}", iconset.display()))?;
    }
    Cmd::new("iconutil")
        .args(["--convert", "iconset", "--output"])
        .arg(&iconset)
        .arg(app.join("Contents/Resources/icon.icns"))
        .run()?;
    Cmd::new("ls").arg(&iconset).run()?;
    for size in ICON_SIZES {
        let file = iconset.join(format!("icon_{size}.png"));
        ensure(fs::metadata(&file).is_ok_and(|m| m.len() > 0), format!("the icon has no {size}"))?;
    }
    let largest = Cmd::new("sips").args(["-g", "pixelWidth"]).arg(iconset.join("icon_512x512@2x.png")).stdout()?;
    ensure(largest.contains("1024"), "the largest icon is not 1024 pixels wide")?;
    // How macOS itself decodes every entry, for the record (uploaded with the .dmg window screenshots).
    let record = repo().join("dmg-screens/icon.iconset");
    if record.exists() {
        fs::remove_dir_all(&record).map_err(|e| format!("{}: {e}", record.display()))?;
    }
    copy_dir(&iconset, &record)?;
    ensure(plist(&app, "CFBundleName")? == "Sidevoice", "CFBundleName is not Sidevoice")?;

    eprintln!("--- architecture");
    let archs = Cmd::new("lipo").arg("-archs").arg(binary()).stdout()?;
    eprintln!("{archs}");
    ensure(archs.contains("arm64"), "the binary is not arm64")?;

    eprintln!("--- no CI probe in a release (desktop/src/probe.rs is behind the probe feature)");
    let bytes = read_bytes(&binary())?;
    for marker in PROBE_ONLY {
        ensure(!contains(&bytes, marker), format!("the release app carries {marker}: a CI probe is in it"))?;
    }
    Ok(())
}

const ICON_SIZES: [&str; 10] = [
    "16x16",
    "16x16@2x",
    "32x32",
    "32x32@2x",
    "128x128",
    "128x128@2x",
    "256x256",
    "256x256@2x",
    "512x512",
    "512x512@2x",
];

/// `release package [--signed]`: the .dmg, `dist/Sidevoice_<version>_aarch64.dmg`, with the brand's window (background,
/// icon layout and volume icon: scripts/dmg-settings.py, dmgbuild), verified, and the app checked as shipped inside
/// it. Without `--signed`, docs/FIRST_OPEN.txt goes beside it (`dist/LEEME-primera-apertura.txt`): how to open an app
/// Gatekeeper does not know.
pub fn dmg(signed: bool) -> Result<()> {
    let work = work("dmg")?;
    // No older image beside this one, to be checksummed or uploaded with it.
    let dist = repo().join("dist");
    mkdir(&dist)?;
    for old in crate::util::files(&dist)?.into_iter().filter(|p| crate::util::name(p).ends_with(".dmg")) {
        fs::remove_file(&old).map_err(|e| format!("{}: {e}", old.display()))?;
    }
    let dmg = dmg_path()?;
    assemble_disk_image(
        &work,
        &app(),
        [
            Path::new("desktop/packaging/macos/dmg/background.png"),
            Path::new("desktop/packaging/macos/dmg/background@2x.png"),
        ],
        Path::new("desktop/packaging/macos/dmg/layout.json"),
        Path::new("desktop/packaging/macos/icon.icns"),
        &dmg,
    )?;
    Cmd::new("hdiutil").arg("verify").arg(&dmg).run()?;
    // Check the app as shipped, inside the image, not the one in the build tree.
    let mount = Mount::attach(&dmg, &work.join("mount"))?;
    let mnt = &mount.path;
    Cmd::new("ls").arg("-la").arg(mnt).run()?;
    for name in [".background.tiff", ".DS_Store", ".VolumeIcon.icns"] {
        ensure(fs::metadata(mnt.join(name)).is_ok_and(|m| m.len() > 0), format!("the .dmg has no {name}"))?;
    }
    let applications = fs::symlink_metadata(mnt.join("Applications"));
    ensure(applications.is_ok_and(|m| m.file_type().is_symlink()), "the .dmg has no Applications link")?;
    let inside = mnt.join("Sidevoice.app");
    Cmd::new("codesign").args(["--verify", "--deep", "--strict", "--verbose=2"]).arg(&inside).run()?;
    ensure(
        entitlements(&inside)?.contains("com.apple.security.device.audio-input"),
        "the app in the .dmg has no microphone entitlement",
    )?;
    eprintln!("{}", plist(&inside, "NSMicrophoneUsageDescription")?);
    mount.detach()?;
    if !signed {
        let readme = repo().join("dist/LEEME-primera-apertura.txt");
        fs::copy(repo().join("docs/FIRST_OPEN.txt"), &readme).map_err(|e| format!("{}: {e}", readme.display()))?;
    }
    Ok(())
}

/// A disk image attached read-only at a mount point of its own, not shown in Finder; detached when dropped.
struct Mount {
    path: PathBuf,
    attached: bool,
}

impl Mount {
    fn attach(dmg: &Path, path: &Path) -> Result<Mount> {
        fresh_dir(path)?;
        Cmd::new("hdiutil").args(["attach", "-readonly", "-nobrowse", "-mountpoint"]).arg(path).arg(dmg).run()?;
        Ok(Mount { path: path.to_owned(), attached: true })
    }

    fn detach(mut self) -> Result<()> {
        Cmd::new("hdiutil").arg("detach").arg(&self.path).run()?;
        self.attached = false;
        Ok(())
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        if self.attached {
            let _ = Cmd::new("hdiutil").arg("detach").arg(&self.path).arg("-force").attempt();
        }
    }
}

/// A keychain of this step's own, removed with its directory when dropped.
struct Keychain {
    path: PathBuf,
    password: String,
    dir: SecretDir,
}

impl Keychain {
    /// A new keychain holding the Developer ID certificate and key (`APPLE_CERTIFICATE`, a base64 .p12, with
    /// `APPLE_CERTIFICATE_PASSWORD`), which codesign may use.
    fn with_certificate() -> Result<Keychain> {
        let certificate = required_env("APPLE_CERTIFICATE")?;
        let certificate_password = required_env("APPLE_CERTIFICATE_PASSWORD")?;
        let dir = SecretDir::new(SIGNING)?;
        let keychain = Keychain { path: dir.path.join(KEYCHAIN), password: random_hex(24)?, dir };
        let (path, password) = (&keychain.path, &keychain.password);
        create_keychain(path, password)?.output()?;
        Cmd::new("security").args(["set-keychain-settings", "-lut", "3600"]).arg(path).run()?;
        unlock_keychain(path, password)?.output()?;
        let p12 = keychain.dir.path.join("certificate.p12");
        Cmd::new("base64").arg("--decode").pipe(
            certificate.as_bytes(),
            Files::open_private(&p12, Create::New).map_err(|e| format!("{}: {e}", p12.display()))?,
        )?;
        let imported = import_certificate(&p12, path, &certificate_password).and_then(Cmd::output).map(drop);
        let _ = fs::remove_file(&p12);
        imported?;
        allow_codesign(path, password)?.output()?;
        // `security -i` need not fail when a command does, so the keychain itself says whether the import worked.
        let identities = Cmd::new("security").args(["find-identity", "-p", "codesigning"]).arg(path).stdout()?;
        ensure(
            !identities.contains(" 0 identities found"),
            "the certificate was not imported into the signing keychain",
        )?;
        // codesign takes the identity from this keychain (`--keychain`), but reaches its key and certificate chain
        // through the user's search list: off the list, signing fails with "The specified item could not be found in
        // the keychain". So it goes on the list, as the bundler's own does; deleting it takes it off.
        let mut keychains = user_keychains()?;
        keychains.push(path.display().to_string());
        set_user_keychains(&keychains)?;
        Ok(keychain)
    }
}

impl Drop for Keychain {
    fn drop(&mut self) {
        let _ = Cmd::new("security").arg("delete-keychain").arg(&self.path).attempt();
    }
}

/// The user's keychain search list set to `keychains`.
fn set_user_keychains(keychains: &[String]) -> Result<()> {
    Cmd::new("security").args(["list-keychains", "-d", "user", "-s"]).args(keychains).run()
}

/// `release sign-package`: the .dmg signed with the Developer ID (`APPLE_SIGNING_IDENTITY`; from a keychain of this
/// step's own holding `APPLE_CERTIFICATE`, or on a laptop without it, from the login keychain), notarized with
/// `notarytool` (`APPLE_API_KEY`, `APPLE_API_ISSUER` and the key file) and stapled, so Gatekeeper opens it from a
/// download without a word; then the image and the app inside it checked as a download has them. The keychain, the
/// certificate file and the key file are removed when it ends, whatever happened.
pub fn sign_dmg() -> Result<()> {
    forget_secrets()?;
    let _private = PrivateFiles::start();
    let identity = required_env("APPLE_SIGNING_IDENTITY")?;
    let key_id = required_env("APPLE_API_KEY")?;
    let issuer = required_env("APPLE_API_ISSUER")?;
    let dmg = dmg_path()?;
    ensure(dmg.is_file(), format!("{} is not there: `cargo xtask release package --signed` first", dmg.display()))?;
    let keychain = match std::env::var("APPLE_CERTIFICATE") {
        Ok(certificate) if !certificate.is_empty() => Some(Keychain::with_certificate()?),
        _ if !in_ci() => None,
        _ => return Err(required_env("APPLE_CERTIFICATE").unwrap_err()),
    };
    let secrets = SecretDir::new(NOTARY)?;
    let key = notary_key(&secrets)?;

    let mut sign = Cmd::new("codesign").arg("--sign").secret(&identity);
    if let Some(keychain) = &keychain {
        sign = sign.arg("--keychain").arg(&keychain.path);
    }
    sign.arg("--timestamp").arg(&dmg).run()?;
    let answer = Cmd::new("xcrun")
        .args(["notarytool", "submit"])
        .arg(&dmg)
        .arg("--key")
        .arg(&key)
        .arg("--key-id")
        .secret(&key_id)
        .arg("--issuer")
        .secret(&issuer)
        .args(["--wait", "--timeout", "30m", "--output-format", "json"])
        .stdout()?;
    eprintln!("{answer}");
    let status = notary_status(&answer)?;
    ensure(status == "Accepted", format!("notarization: {status}"))?;
    stapler("staple", &dmg)?;
    stapler("validate", &dmg)?;
    let spctl = Cmd::new("spctl")
        .args(["-a", "-vv", "-t", "open", "--context", "context:primary-signature"])
        .arg(&dmg)
        .output()?;
    eprintln!("{spctl}");
    ensure(notarized(&spctl), "Gatekeeper does not take the .dmg as a notarized Developer ID image")?;
    // The app inside, as a download would have it: notarized, stapled, accepted.
    let mount = Mount::attach(&dmg, &work("sign-dmg")?.join("mount"))?;
    let inside = mount.path.join("Sidevoice.app");
    stapler("validate", &inside)?;
    let spctl = Cmd::new("spctl").args(["-a", "-vv", "-t", "exec"]).arg(&inside).output()?;
    ensure(notarized(&spctl), "Gatekeeper does not take the app in the .dmg as a notarized Developer ID app")?;
    mount.detach()
}

/// Where the signing steps keep their secrets ([`SecretDir`]): the keychain and the certificate, the notary's key.
const NOTARY: &str = "sidevoice-notary";

/// `release forget-secrets`: whatever a signing step that was cut short (cancelled, killed) left: its keychain (and its
/// place on the search list), the certificate file, the notary's key file. Each step removes its own when it ends; CI
/// also runs this at the end of the job, whatever happened.
pub fn forget_secrets() -> Result<()> {
    let signing = secret_path(SIGNING);
    let _ = Cmd::new("security").arg("delete-keychain").arg(signing.join(KEYCHAIN)).attempt();
    let keychains = user_keychains()?;
    if keychains.iter().any(|k| signing_keychain(k)) {
        let kept: Vec<String> = keychains.into_iter().filter(|k| !signing_keychain(k)).collect();
        set_user_keychains(&kept)?;
    }
    for dir in [signing, secret_path(NOTARY)] {
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    Ok(())
}

/// `release checksums`: `dist/SHA256SUMS.txt`, the SHA-256 of each .dmg in dist/ (`shasum -a 256 *.dmg`).
pub fn checksums() -> Result<()> {
    let dist = repo().join("dist");
    let dmgs: Vec<PathBuf> =
        crate::util::files(&dist)?.into_iter().filter(|p| crate::util::name(p).ends_with(".dmg")).collect();
    ensure(!dmgs.is_empty(), "no .dmg in dist/")?;
    let sums = crate::publish::sums(&dmgs)?;
    print!("{sums}");
    crate::util::write(&dist.join("SHA256SUMS.txt"), sums)
}

/// `release package-screens` (informational): the .dmg's window as Finder shows it, light and dark, into dmg-screens/.
/// AppleScript calls are cut off after 30 seconds (a permission prompt must not hang it); nothing here fails it but a
/// missing .dmg.
pub fn dmg_screens() -> Result<()> {
    let screens = repo().join("dmg-screens");
    mkdir(&screens)?;
    let dmg = dmg_path()?;
    let attached = Cmd::new("hdiutil").args(["attach", "-readonly"]).arg(&dmg).output()?;
    eprintln!("{attached}");
    let volume = crate::util::re("/Volumes/.*").find(&attached).map(|m| m.as_str().to_owned()).unwrap_or_default();
    // The appearance as it was, put back at the end (it matters on a laptop).
    let dark = osa(&["tell application \"System Events\" to tell appearance preferences to get dark mode"]);
    for mode in ["false", "true"] {
        let _ = osa(&[&format!(
            "tell application \"System Events\" to tell appearance preferences to set dark mode to {mode}"
        )]);
        let _ = osa(&[
            "tell application \"Finder\" to close every window",
            &format!("tell application \"Finder\" to open (POSIX file \"{volume}\" as alias)"),
            "tell application \"Finder\" to activate",
        ]);
        sleep(4.0);
        let bounds =
            osa(&["tell application \"Finder\" to get bounds of front window"]).unwrap_or_default().replace(' ', "");
        let bounds = bounds.trim();
        eprintln!("window bounds ({mode}): {bounds}");
        let shot = screens.join(format!("dmg-dark-{mode}.png"));
        let region = region(if bounds.is_empty() { "0,28,1440,900" } else { bounds });
        let taken = region.is_some_and(|region| {
            Cmd::new("screencapture").arg("-x").arg(format!("-R{region}")).arg(&shot).run().is_ok()
        });
        if !taken {
            let _ = Cmd::new("screencapture").arg("-x").arg(screens.join(format!("dmg-dark-{mode}-full.png"))).run();
        }
    }
    let _ = osa(&["tell application \"Finder\" to close every window"]);
    if let Ok(dark) = dark {
        let dark = dark.trim();
        let _ = osa(&[&format!(
            "tell application \"System Events\" to tell appearance preferences to set dark mode to {dark}"
        )]);
    }
    let _ = Cmd::new("hdiutil").arg("detach").arg(&volume).run();
    Cmd::new("ls").arg("-la").arg(&screens).run()
}

/// `osascript -e …`, cut off after 30 seconds; its output.
fn osa(lines: &[&str]) -> Result<String> {
    let mut cmd = Cmd::new("osascript");
    for line in lines {
        cmd = cmd.arg("-e").arg(line);
    }
    let log = work("dmg-screens")?.join("osascript.out");
    let mut running = cmd.spawn(&log)?;
    for _ in 0..300 {
        if !running.running() {
            return Ok(read(&log));
        }
        sleep(0.1);
    }
    running.stop();
    Err("osascript did not answer in 30 seconds".into())
}

/// `xcrun stapler <action> <path>`, retried: both actions ask Apple's ticket service (CloudKit), and a runner's
/// connection to it drops now and then ("The network connection was lost", error 68) though the ticket is fine.
fn stapler(action: &str, path: impl AsRef<Path>) -> Result<()> {
    stapler_waiting(action, path, &[15.0, 30.0, 60.0])
}

/// How long [`staple_app`] waits for Apple's ticket between its attempts (seconds): about eight minutes in all.
const STAPLE_WAITS: [f64; 6] = [15.0, 30.0, 60.0, 120.0, 120.0, 120.0];

/// [`stapler`] with `waits` between its attempts.
fn stapler_waiting(action: &str, path: impl AsRef<Path>, waits: &[f64]) -> Result<()> {
    let path = path.as_ref();
    for (attempt, wait) in waits.iter().enumerate() {
        match Cmd::new("xcrun").args(["stapler", action]).arg(path).run() {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("xtask: {e}; retrying in {wait} s ({} of {})", attempt + 1, waits.len());
                crate::util::sleep(*wait);
            }
        }
    }
    Cmd::new("xcrun").args(["stapler", action]).arg(path).run()
}

/// Writes `output` from the `.app` bundle `app` with `dmgbuild` (pinned, in a virtualenv of its own) and a `tiffutil`
/// background, as scripts/dmg-settings.py expects: `background` is the window drawing at 1x and at 2x, `layout` the
/// icon layout (`layout.json`) and `icon` the volume icon. `work` is a scratch directory the build may fill.
fn assemble_disk_image(
    work: &Path,
    app: &Path,
    background: [&Path; 2],
    layout: &Path,
    icon: &Path,
    output: &Path,
) -> Result<()> {
    let venv = work.join("dmgbuild");
    Cmd::new("python3").args(["-m", "venv"]).arg(&venv).run()?;
    Cmd::new(venv.join("bin/pip")).args(["install", "--quiet", "dmgbuild==1.6.7"]).run()?;
    // One TIFF with the 1x and 2x drawings: Finder picks the one for the screen.
    let tiff = work.join("background.tiff");
    Cmd::new("tiffutil").arg("-cathidpicheck").args(background).arg("-out").arg(&tiff).run()?;
    Cmd::new(venv.join("bin/dmgbuild"))
        .args(["-s", "scripts/dmg-settings.py"])
        .arg("-D")
        .arg(format!("app={}", app.display()))
        .arg("-D")
        .arg(format!("background={}", tiff.display()))
        .arg("-D")
        .arg(format!("layout={}", layout.display()))
        .arg("-D")
        .arg(format!("icon={}", icon.display()))
        .arg("Sidevoice")
        .arg(output)
        .run()
}
