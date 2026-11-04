//! `mobile testflight --build-number N`: the iOS app archived, signed, exported for App Store Connect and uploaded to
//! TestFlight (`.github/workflows/mobile-testflight.yml`, which runs it by hand, from main).
//!
//! - **Signing is Xcode's, automatic.** The Tauri CLI archives with `xcodebuild -allowProvisioningUpdates` and exports
//!   with the App Store Connect API key (`-authenticationKeyID`, `-authenticationKeyIssuerID`, `-authenticationKeyPath`),
//!   which has Xcode create or fetch the Apple Distribution certificate (cloud-managed) and the App Store provisioning
//!   profile for the bundle identifier, and register the identifier if the account lacks it. The key needs the Admin role
//!   for that. Nothing is imported into a keychain.
//! - **The build number is `N`** (CI: the workflow's run number, which only grows), the app's `CFBundleVersion` as a
//!   single integer: the CLI's own `--build-number` appends it to the version (`0.1.0.7`), four numbers where Apple
//!   accepts three. The CLI's `bundle > iOS > bundleVersion`, given for this build only, sets it whole.
//! - **The archive is read before it is sent** ([`ios::check_ipa`]): the app, the build number, signed, the privacy
//!   manifest and the encryption declaration.
//! - **The key** (`APPLE_API_KEY_P8`, the .p8's contents) is written to a file only this user can read, in a directory
//!   of its own, and removed when the command ends, however it ends. The contents are taken out of this process's
//!   environment as they are read, before any child is started, so no child (the Tauri CLI, `xcodebuild`, `agvtool`,
//!   `altool`, and what they start) can see them; they get the key's id, its issuer and the file's path. Nothing here
//!   prints one of the three.
//!
//! - **The symbols are kept** ([`keep_symbols`]): the archive's dSYMs and the app's executable, zipped into dist/ for the
//!   workflow to keep, before the upload; and the archive sent carries them for App Store Connect (`Symbols/`, which
//!   [`ios::check_ipa`] requires), so TestFlight's crash reports are symbolicated. The Rust code's names are in them:
//!   the app links the Rust library as a static library, which the release profile's `strip` does not touch, and only
//!   the archive's own last step strips the executable, after Xcode has written the dSYM.
//!
//! Reads `APPLE_API_KEY` (the key's id), `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` and `APPLE_TEAM_ID`.
//!
//! `mobile symbols --build-number N` is the same archive unsigned and not uploaded, for its symbols alone: those of a
//! build made before they were kept, rebuilt from the build's commit (.github/workflows/mobile.yml, `symbols-of-build`).
//! It is made exactly as the signed one (same target, profile, build number and engine libraries), so its code is laid
//! out the same and a crash report's offsets into the shipped executable read in it.

use std::fs;
use std::path::{Path, PathBuf};

use super::{build_tauri, ios};
use crate::platform::ios_upload;
use crate::util::{ensure, in_ci, mkdir, repo, required_env, Cmd, SecretDir};
use crate::Result;

/// The directory of the key's file (below `RUNNER_TEMP` on a runner).
const SECRETS: &str = "sidevoice-testflight";

/// Where the Tauri CLI leaves what it builds, below mobile/tauri/gen/apple.
const BUILD: &str = "build";

/// A signed build runs from `main` only. The workflow's job says so too; this holds for the command on its own.
const MAIN: &str = "refs/heads/main";

/// The archive the Tauri CLI leaves in [`BUILD`]: the Xcode scheme's name (mobile/tauri/gen/apple/project.yml).
const ARCHIVE: &str = "sidevoice-mobile_iOS.xcarchive";

/// The app's executable inside the archive.
const EXECUTABLE: &str = "Products/Applications/Sidevoice.app/Sidevoice";

/// The app's own dSYM inside the archive, beside the extension's and the engine frameworks'.
const APP_DSYM: &str = "dSYMs/Sidevoice.app.dSYM/Contents/Resources/DWARF/Sidevoice";

/// The header of an `.ips` crash report is a JSON line; `slice_uuid` is the app executable's build UUID.
const IPS_SLICE_UUID: &str = "slice_uuid";

/// A name the Rust library always has (the standard library's panic entry): the dSYM has the Rust code's symbols when
/// it names it.
const RUST_SYMBOL: &[u8] = b"rust_begin_unwind";

/// The bundle identifier the archive must be (mobile/tauri/tauri.conf.json).
pub fn identifier() -> Result<String> {
    let path = repo().join("mobile/tauri/tauri.conf.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    config["identifier"].as_str().map(str::to_owned).ok_or_else(|| format!("{}: no identifier", path.display()))
}

/// The build number given: a whole number of at least 1.
fn build_number(text: &str) -> Result<u32> {
    text.parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("the build number is a whole number from 1, not {text:?}"))
}

/// The one `.ipa` below `dir`, at most three levels down (`build/arm64/Sidevoice.ipa`).
fn find_ipa(dir: &Path, depth: usize) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.is_dir() && depth > 1 {
            found.extend(find_ipa(&path, depth - 1)?);
        } else if path.extension().is_some_and(|e| e == "ipa") {
            found.push(path);
        }
    }
    Ok(found)
}

/// The variable `name`, which must be set and not empty, taken out of this process's environment: every child started
/// after this inherits the environment as it is, so none gets the value. It is never printed.
fn take_secret(name: &str) -> Result<String> {
    let value = required_env(name)?;
    std::env::remove_var(name);
    Ok(value)
}

/// `mobile testflight --build-number N`.
pub fn testflight(build_number_text: &str) -> Result<()> {
    let build = build_number(build_number_text)?;
    if in_ci() {
        let git_ref = std::env::var("GITHUB_REF").unwrap_or_default();
        ensure(git_ref == MAIN, format!("a signed build runs from main only, not from {git_ref:?}"))?;
    }
    let key_id = required_env("APPLE_API_KEY")?;
    let issuer = required_env("APPLE_API_ISSUER")?;
    let team = required_env("APPLE_TEAM_ID")?;
    let p8 = take_secret("APPLE_API_KEY_P8")?;
    let identifier = identifier()?;

    // `private_keys/AuthKey_<id>.p8` is where altool looks, beside the directory it runs in; the CLI takes the path.
    let secrets = SecretDir::new(SECRETS)?;
    let key = secrets.file(&format!("private_keys/AuthKey_{key_id}.p8"), format!("{p8}\n").as_bytes())?;
    drop(p8);

    archive(build, &["--export-method", "app-store-connect"], |cli| {
        cli.env("APPLE_API_KEY", &key_id)
            .env("APPLE_API_ISSUER", &issuer)
            .env("APPLE_API_KEY_PATH", &key)
            .env("APPLE_DEVELOPMENT_TEAM", &team)
    })?;
    keep_symbols(build)?;

    let out = repo().join("mobile/tauri/gen/apple").join(BUILD);
    let ipas = find_ipa(&out, 3)?;
    ensure(ipas.len() == 1, format!("expected one .ipa below {}, found {}", out.display(), ipas.len()))?;
    ios::check_ipa(&ipas[0], &identifier, build)?;
    ios_upload::upload(&ipas[0], &key_id, &issuer, &secrets.path)?;
    println!("{identifier} build {build} uploaded: App Store Connect processes it, then TestFlight offers it to the internal testers");
    Ok(())
}

/// The app archived for a device as build `build`, into [`BUILD`] (emptied first): the Tauri CLI's `ios build` for
/// `aarch64` in the release profile, with `how` (the export, or `--no-sign`) and `sign` (the signing variables, or none).
/// The signed archive and the symbols' rebuild are this one build, so that their code is the same.
fn archive(build: u32, how: &[&str], sign: impl FnOnce(Cmd) -> Cmd) -> Result<()> {
    let out = repo().join("mobile/tauri/gen/apple").join(BUILD);
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    let config = serde_json::json!({ "bundle": { "iOS": { "bundleVersion": build.to_string() } } }).to_string();
    let mut args = vec!["ios", "build", "--target", "aarch64"];
    args.extend(how);
    args.extend(["--config", &config, "--ci"]);
    let libs = super::AppBuildLibs::hold("ios", &["aarch64-apple-ios"])?;
    sign(libs.libs.apply(build_tauri(&args)?)).run()?;
    drop(libs);
    Ok(())
}

/// What reads a crash report of build `build`, from the archive in [`BUILD`]: its dSYMs (the app's, the Live Activity
/// extension's, the engine frameworks') and the app's executable as it ships, whose code a report's offsets point into.
/// Zipped into dist/ as `Sidevoice-ios-<build>-symbols.zip`, which the workflows keep. The app's dSYM must name the Rust
/// code ([`RUST_SYMBOL`]): an archive whose crashes in Rust would read as bare offsets is refused.
pub fn keep_symbols(build: u32) -> Result<PathBuf> {
    let archive = repo().join("mobile/tauri/gen/apple").join(BUILD).join(ARCHIVE);
    let dsym = archive.join(APP_DSYM);
    let bytes = fs::read(&dsym).map_err(|e| format!("{}: {e}", dsym.display()))?;
    ensure(
        bytes.windows(RUST_SYMBOL.len()).any(|w| w == RUST_SYMBOL),
        format!("{}: no Rust symbols (no {})", dsym.display(), String::from_utf8_lossy(RUST_SYMBOL)),
    )?;
    let dist = repo().join("dist");
    mkdir(&dist)?;
    let zip = dist.join(format!("Sidevoice-ios-{build}-symbols.zip"));
    if zip.exists() {
        fs::remove_file(&zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    }
    // A dSYM is a directory: zip keeps its tree (`-r`), and the executable goes at the root (`-j`).
    Cmd::new("zip").args(["-qry"]).arg(&zip).arg("dSYMs").dir(&archive).run()?;
    Cmd::new("zip").args(["-qj"]).arg(&zip).arg(archive.join(EXECUTABLE)).run()?;
    println!("{}: the symbols of build {build}", zip.display());
    Ok(zip)
}

/// The build UUIDs (upper case, no dashes) that `dwarfdump --uuid` prints for `path`: one per architecture slice.
fn uuids(path: &Path) -> Result<Vec<String>> {
    let text = Cmd::new("dwarfdump").arg("--uuid").arg(path).stdout()?;
    println!("{}", text.trim_end());
    Ok(parse_uuids(&text))
}

/// The UUIDs in `dwarfdump --uuid` output (`UUID: <uuid> (arm64) <path>`).
fn parse_uuids(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("UUID:"))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(|uuid| uuid.replace('-', "").to_ascii_uppercase())
        .collect()
}

/// The `slice_uuid` (upper case, no dashes) in the header line of the `.ips` crash report `text`.
fn ips_slice_uuid(text: &str) -> Result<String> {
    let header: serde_json::Value =
        serde_json::from_str(text.lines().next().unwrap_or_default()).map_err(|e| format!("the .ips header: {e}"))?;
    let uuid = header.get(IPS_SLICE_UUID).and_then(|value| value.as_str());
    uuid.map(|uuid| uuid.replace('-', "").to_ascii_uppercase())
        .ok_or_else(|| format!("the .ips header has no {IPS_SLICE_UUID}"))
}

/// Prints the UUIDs of the rebuilt executable and app dSYM, which must be those of the build that crashed: the rebuild
/// is not bit-identical unless Xcode and the toolchain are the original's, and symbols of another binary read as
/// plausible, wrong names. With `ips` (a crash report of the build), fails unless they match its `slice_uuid`.
fn check_uuids(archive: &Path, ips: Option<&Path>) -> Result<()> {
    println!("UUIDs of the rebuilt binary and dSYM (compare with the crash report's slice_uuid before trusting the symbols):");
    let executable = uuids(&archive.join(EXECUTABLE))?;
    let dsym = uuids(&archive.join(APP_DSYM))?;
    ensure(executable == dsym, format!("the executable {executable:?} and its dSYM {dsym:?} differ"))?;
    let Some(ips) = ips else { return Ok(()) };
    let text = fs::read_to_string(ips).map_err(|e| format!("{}: {e}", ips.display()))?;
    let wanted = ips_slice_uuid(&text)?;
    ensure(
        executable.contains(&wanted),
        format!("the crash report's slice_uuid {wanted} is not the rebuild's {executable:?}: these symbols are NOT of that build"),
    )?;
    println!("the rebuild matches the crash report's slice_uuid: these symbols are of that build");
    Ok(())
}

/// `mobile symbols --build-number N`: the archive [`testflight`] makes for build `N`, unsigned and not uploaded, and its
/// symbols ([`keep_symbols`]). Needs no secret: it is for reading the crash reports of a build that kept none. Xcode is
/// not pinned, so the rebuild may differ from the shipped binary: it prints the UUIDs of what it built ([`check_uuids`]),
/// and checks them against `ips`, a crash report of the build, when given.
pub fn symbols(build_number_text: &str, ips: Option<&Path>) -> Result<()> {
    let build = build_number(build_number_text)?;
    archive(build, &["--no-sign"], |cli| cli)?;
    check_uuids(&repo().join("mobile/tauri/gen/apple").join(BUILD).join(ARCHIVE), ips)?;
    keep_symbols(build).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_number_is_a_whole_number_from_one() {
        assert_eq!(build_number("7").unwrap(), 7);
        assert_eq!(build_number("1234").unwrap(), 1234);
        for bad in ["0", "-1", "1.2", "0.1.0.7", "", "x", "99999999999"] {
            assert!(build_number(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_secret_is_taken_out_of_the_environment_for_every_child_after_it() {
        let name = "SIDEVOICE_XTASK_TEST_TAKEN_SECRET";
        std::env::set_var(name, "fixture-secret");
        assert_eq!(take_secret(name).unwrap(), "fixture-secret");
        assert!(std::env::var_os(name).is_none(), "a child would inherit it");
        assert!(take_secret(name).unwrap_err().contains("is not set"));
        std::env::set_var(name, "");
        assert!(take_secret(name).is_err(), "an empty secret is none");
        assert!(std::env::var_os(name).is_some(), "nothing taken, nothing removed");
        std::env::remove_var(name);
    }

    #[test]
    fn the_archive_is_found_a_few_levels_down() {
        let dir = tempfile::tempdir().unwrap();
        let ipa = dir.path().join("arm64/Sidevoice.ipa");
        fs::create_dir_all(ipa.parent().unwrap()).unwrap();
        fs::write(&ipa, b"x").unwrap();
        fs::write(dir.path().join("arm64/ExportOptions.plist"), b"x").unwrap();
        assert_eq!(find_ipa(dir.path(), 3).unwrap(), [ipa]);
        assert!(find_ipa(dir.path(), 1).unwrap().is_empty(), "deeper than asked");
    }

    #[test]
    fn the_uuids_are_read_from_dwarfdump_and_the_crash_report() {
        let out = "UUID: 1A2B3C4D-0000-1111-2222-333344445555 (arm64) /x/Sidevoice\n";
        assert_eq!(parse_uuids(out), ["1A2B3C4D000011112222333344445555"]);
        let ips = "{\"app_name\":\"Sidevoice\",\"slice_uuid\":\"1a2b3c4d-0000-1111-2222-333344445555\"}\n{\"usedImages\":[]}";
        assert_eq!(ips_slice_uuid(ips).unwrap(), "1A2B3C4D000011112222333344445555");
        assert!(ips_slice_uuid("{}\n").is_err());
    }

    #[test]
    fn the_identifier_is_the_apps() {
        assert_eq!(identifier().unwrap(), "ai.sidevoice.mobile");
    }
}
