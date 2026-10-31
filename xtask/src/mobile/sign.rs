//! The Android app signed with the upload key, and the 16 KB page check Google Play makes of its native libraries.
//!
//! - `mobile sign android`: dist/Sidevoice-android-arm64-unsigned.apk (`mobile build android`) aligned (`zipalign`,
//!   16 KB pages for the uncompressed libraries, 4 bytes for the rest), signed with `apksigner` (v2 and later schemes)
//!   with the upload key, verified, and checked ([`check_16kb`]), into dist/Sidevoice-android-arm64.apk. Reads
//!   `ANDROID_UPLOAD_KEY_ALIAS`, `ANDROID_UPLOAD_KEYSTORE_PASSWORD` and the PKCS#12 keystore: `ANDROID_UPLOAD_KEYSTORE_B64`
//!   (its contents, base64) or, on a laptop, `ANDROID_UPLOAD_KEYSTORE_PATH` (the file). The keystore and its password go
//!   into files only this user can read, removed when the command ends; the password never reaches a command line.
//! - `mobile check-16kb [APK]`: the APK (default: the signed one) has its uncompressed libraries on 16 KB boundaries
//!   and every library's loadable segments aligned to 16 KB, which Google Play requires of apps that target Android 15
//!   or later on 64-bit devices.
//!
//! The tools are the Android SDK's build-tools (`ANDROID_HOME`, the newest installed).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine;

use crate::util::{ensure, read_secret, repo, work, Cmd, SecretDir};
use crate::Result;

/// The page size Google Play asks of native libraries: 16 KB.
const PAGE: u64 = 16 * 1024;

fn unsigned() -> PathBuf {
    repo().join("dist/Sidevoice-android-arm64-unsigned.apk")
}

fn signed() -> PathBuf {
    repo().join("dist/Sidevoice-android-arm64.apk")
}

/// A tool of the newest build-tools installed in `ANDROID_HOME`.
fn build_tool(name: &str) -> Result<PathBuf> {
    let home = crate::util::required_env("ANDROID_HOME")?;
    let dir = Path::new(&home).join("build-tools");
    let version = |name: &std::ffi::OsStr| -> Vec<u32> {
        name.to_string_lossy().split('.').map(|part| part.parse().unwrap_or(0)).collect()
    };
    let newest = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.file_name()))
        .max_by_key(|name| version(name))
        .ok_or_else(|| format!("{}: no build-tools installed", dir.display()))?;
    Ok(dir.join(newest).join(name))
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
    let input = unsigned();
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
    check_16kb(Some(&out))
}

/// `mobile check-16kb [APK]`.
pub fn check_16kb(apk: Option<&Path>) -> Result<()> {
    let apk = apk.map(Path::to_path_buf).unwrap_or_else(signed);
    ensure(apk.exists(), format!("{}: no such file", apk.display()))?;
    // The libraries stored uncompressed start on a 16 KB boundary, so the system maps them out of the APK.
    let (aligned, listing) =
        Cmd::new(build_tool("zipalign")?).args(["-c", "-P", "16", "-v", "4"]).arg(&apk).attempt()?;
    let misaligned: Vec<_> = listing.lines().filter(|line| line.contains("BAD")).collect();
    ensure(aligned, format!("{} is not aligned to 16 KB pages:\n{}", apk.display(), misaligned.join("\n")))?;
    let file = fs::File::open(&apk).map_err(|e| format!("{}: {e}", apk.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", apk.display()))?;
    let mut checked = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| format!("{}: {e}", apk.display()))?;
        let name = entry
            .name()
            .map(std::borrow::Cow::into_owned)
            .map_err(|e| format!("{}: an entry name: {e}", apk.display()))?;
        if !(name.starts_with("lib/") && name.ends_with(".so")) {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| format!("{name}: {e}"))?;
        for align in load_alignments(&bytes).map_err(|e| format!("{name}: {e}"))? {
            ensure(align >= PAGE, format!("{name}: a loadable segment is aligned to {align} bytes, not {PAGE}"))?;
        }
        checked += 1;
    }
    ensure(checked > 0, format!("{}: no native library in lib/", apk.display()))?;
    println!("{checked} native libraries, loadable segments aligned to 16 KB");
    Ok(())
}

/// The alignment of each loadable segment (`PT_LOAD`) of a 64-bit little-endian ELF file, which every Android ABI the
/// app ships is (arm64-v8a, x86_64).
fn load_alignments(elf: &[u8]) -> Result<Vec<u64>> {
    let at = |offset: usize, len: usize| elf.get(offset..offset + len).ok_or("a truncated ELF file".to_owned());
    let u16_at = |offset| at(offset, 2).map(|b| u16::from_le_bytes(b.try_into().expect("2 bytes")));
    let u32_at = |offset| at(offset, 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
    let u64_at = |offset| at(offset, 8).map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")));
    ensure(elf.starts_with(b"\x7fELF"), "not an ELF file")?;
    ensure(elf.get(4) == Some(&2) && elf.get(5) == Some(&1), "not a 64-bit little-endian ELF file")?;
    let (table, size, count) = (u64_at(0x20)? as usize, u16_at(0x36)? as usize, u16_at(0x38)? as usize);
    const PT_LOAD: u32 = 1;
    let mut alignments = Vec::new();
    for index in 0..count {
        let header = table + index * size;
        if u32_at(header)? == PT_LOAD {
            alignments.push(u64_at(header + 0x30)?);
        }
    }
    ensure(!alignments.is_empty(), "no loadable segment")?;
    Ok(alignments)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ELF64 header with the given program headers `(type, align)`.
    fn elf(headers: &[(u32, u64)]) -> Vec<u8> {
        let mut bytes = vec![0u8; 64 + 56 * headers.len()];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
        bytes[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        bytes[0x38..0x3a].copy_from_slice(&(headers.len() as u16).to_le_bytes());
        for (index, (kind, align)) in headers.iter().enumerate() {
            let at = 64 + 56 * index;
            bytes[at..at + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[at + 0x30..at + 0x38].copy_from_slice(&align.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn the_loadable_segments_alignments_are_read() {
        // PT_LOAD, PT_DYNAMIC (not loadable: its alignment is no matter), PT_LOAD.
        assert_eq!(load_alignments(&elf(&[(1, 16384), (2, 8), (1, 65536)])).unwrap(), [16384, 65536]);
        assert_eq!(load_alignments(&elf(&[(1, 4096)])).unwrap(), [4096], "4 KB is read as it is, the check refuses it");
    }

    #[test]
    fn what_is_not_a_64_bit_elf_is_refused() {
        assert!(load_alignments(b"PK\x03\x04").unwrap_err().contains("not an ELF"));
        let mut thirty_two = elf(&[(1, 4096)]);
        thirty_two[4] = 1;
        assert!(load_alignments(&thirty_two).unwrap_err().contains("64-bit"));
        assert!(load_alignments(&elf(&[(2, 8)])).unwrap_err().contains("no loadable"));
        assert!(load_alignments(&elf(&[(1, 4096)])[..70]).unwrap_err().contains("truncated"));
    }
}
