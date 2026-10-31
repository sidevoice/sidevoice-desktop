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
//!   of its own, and removed when the command ends, however it ends. The child processes get the key's id, its issuer
//!   and the file's path, and never the contents. Nothing here prints one of the three.
//!
//! Reads `APPLE_API_KEY` (the key's id), `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` and `APPLE_TEAM_ID`.

use std::fs;
use std::path::{Path, PathBuf};

use super::{ios, tauri};
use crate::platform::ios_upload;
use crate::util::{ensure, in_ci, repo, required_env, SecretDir};
use crate::Result;

/// The directory of the key's file (below `RUNNER_TEMP` on a runner).
const SECRETS: &str = "sidevoice-testflight";

/// Where the Tauri CLI leaves what it builds, below mobile/gen/apple.
const BUILD: &str = "build";

/// A signed build runs from `main` only. The workflow's job says so too; this holds for the command on its own.
const MAIN: &str = "refs/heads/main";

/// The bundle identifier the archive must be (mobile/tauri.conf.json).
fn identifier() -> Result<String> {
    let path = repo().join("mobile/tauri.conf.json");
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
    let p8 = required_env("APPLE_API_KEY_P8")?;
    let identifier = identifier()?;

    // `private_keys/AuthKey_<id>.p8` is where altool looks, beside the directory it runs in; the CLI takes the path.
    let secrets = SecretDir::new(SECRETS)?;
    let key = secrets.file(&format!("private_keys/AuthKey_{key_id}.p8"), format!("{p8}\n").as_bytes())?;
    drop(p8);

    let out = repo().join("mobile/gen/apple").join(BUILD);
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    let config = serde_json::json!({ "bundle": { "iOS": { "bundleVersion": build.to_string() } } }).to_string();
    tauri(&[
        "ios",
        "build",
        "--target",
        "aarch64",
        "--export-method",
        "app-store-connect",
        "--config",
        &config,
        "--ci",
    ])
    .env("APPLE_API_KEY", &key_id)
    .env("APPLE_API_ISSUER", &issuer)
    .env("APPLE_API_KEY_PATH", &key)
    .env("APPLE_DEVELOPMENT_TEAM", &team)
    .env_remove("APPLE_API_KEY_P8")
    .run()?;

    let ipas = find_ipa(&out, 3)?;
    ensure(ipas.len() == 1, format!("expected one .ipa below {}, found {}", out.display(), ipas.len()))?;
    ios::check_ipa(&ipas[0], &identifier, build)?;
    ios_upload::upload(&ipas[0], &key_id, &issuer, &secrets.path)?;
    println!("{identifier} build {build} uploaded: App Store Connect processes it, then TestFlight offers it to the internal testers");
    Ok(())
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
    fn the_identifier_is_the_apps() {
        assert_eq!(identifier().unwrap(), "ai.sidevoice.mobile");
    }
}
