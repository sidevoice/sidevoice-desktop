//! The Android app signed with the upload key.
//!
//! - `mobile sign android`: dist/Sidevoice-android-arm64-unsigned.apk (`mobile build android`) aligned (`zipalign`,
//!   16 KB pages for the uncompressed libraries, 4 bytes for the rest), signed with `apksigner` (v2 and later schemes)
//!   with the upload key, verified, and checked ([`check_16kb`]), into dist/Sidevoice-android-arm64.apk. Reads
//!   `ANDROID_UPLOAD_KEY_ALIAS`, `ANDROID_UPLOAD_KEYSTORE_PASSWORD` and the PKCS#12 keystore: `ANDROID_UPLOAD_KEYSTORE_B64`
//!   (its contents, base64) or, on a laptop, `ANDROID_UPLOAD_KEYSTORE_PATH` (the file). The keystore and its password go
//!   into files only this user can read, removed when the command ends; the password never reaches a command line.
//!
//! The tools are the Android SDK's build-tools (`ANDROID_HOME`, the newest installed). The 16 KB check is check.rs's.

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;

use super::check::{build_tool, check_16kb};
use super::unsigned_apk;
use crate::util::{ensure, read_secret, repo, work, Cmd, SecretDir};
use crate::Result;

fn signed() -> PathBuf {
    repo().join("dist/Sidevoice-android-arm64.apk")
}

/// The upload keystore's file, in `dir`: the contents `ANDROID_UPLOAD_KEYSTORE_B64` holds, or the file
/// `ANDROID_UPLOAD_KEYSTORE_PATH` names (read through a checked private handle).
fn keystore(dir: &SecretDir) -> Result<PathBuf> {
    match std::env::var("ANDROID_UPLOAD_KEYSTORE_B64") {
        Ok(text) if !text.trim().is_empty() => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(text.split_whitespace().collect::<String>())
                .map_err(|_| "ANDROID_UPLOAD_KEYSTORE_B64 is not base64".to_owned())?;
            dir.file("upload.p12", &bytes)
        }
        _ => match std::env::var_os("ANDROID_UPLOAD_KEYSTORE_PATH") {
            Some(path) => dir.file("upload.p12", &read_secret(Path::new(&path))?),
            None => Err("neither ANDROID_UPLOAD_KEYSTORE_B64 nor ANDROID_UPLOAD_KEYSTORE_PATH (a file) is set".into()),
        },
    }
}

/// `mobile sign android`.
pub fn sign_android() -> Result<()> {
    let (zipalign, apksigner) = (build_tool("zipalign")?, build_tool("apksigner")?);
    let alias = crate::util::required_env("ANDROID_UPLOAD_KEY_ALIAS")?;
    let password = crate::util::required_env("ANDROID_UPLOAD_KEYSTORE_PASSWORD")?;
    let input = unsigned_apk();
    ensure(input.exists(), format!("{}: build it first (`cargo xtask mobile build android`)", input.display()))?;

    let work = work("mobile")?;
    let aligned = work.join("aligned.apk");
    Cmd::new(&zipalign).args(["-P", "16", "-f", "4"]).arg(&input).arg(&aligned).run()?;

    let (out, secrets) = (signed(), SecretDir::new("android-signing")?);
    let (keystore, password) = (keystore(&secrets)?, secrets.file("password", password.as_bytes())?);
    let _ = fs::remove_file(&out);
    Cmd::new(&apksigner)
        .args(["sign", "--v4-signing-enabled", "false", "--ks"])
        .arg(&keystore)
        .args(["--ks-type", "PKCS12", "--ks-key-alias", &alias, "--ks-pass"])
        .arg(format!("file:{}", password.display()))
        .arg("--out")
        .arg(&out)
        .arg(&aligned)
        .run()?;
    drop(secrets);
    let _ = fs::remove_file(&aligned);
    Cmd::new(&apksigner).args(["verify", "--verbose", "--print-certs"]).arg(&out).run()?;
    check_16kb(&out)
}
