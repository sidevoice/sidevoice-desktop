//! Downloading an engine package or a model's files: streamed to disk, SHA-256 checked against the catalog,
//! unpacked into a temporary directory and moved into place only when whole. A directory in place always holds
//! a complete, verified download (its `.sidevoice-complete` marker names the hash it came from).

use sha2::{Digest, Sha256};
use sidevoice_desktop_core::engines::Download;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub type Error = String;
const MARKER: &str = ".sidevoice-complete";

/// Where downloads live: `<root>/engines/…` and `<root>/models/…` (the app's data directory).
#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
}

/// Progress: bytes so far, bytes expected.
pub type OnProgress<'a> = &'a mut dyn FnMut(u64, u64);

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    /// The unpacked root of an engine package, e.g. `engines/sherpa-onnx-1.13.8-macos-aarch64/<root>`.
    pub fn engine_dir(&self, engine: &str, version: &str, os: &str, arch: &str, download: &Download) -> PathBuf {
        self.root.join("engines").join(format!("{engine}-{version}-{os}-{arch}")).join(&download.root)
    }

    /// The unpacked root of a model's files for one engine, e.g. `models/sherpa-onnx/whisper-tiny/<root>`.
    pub fn model_dir(&self, engine: &str, model: &str, download: &Download) -> PathBuf {
        self.root.join("models").join(engine).join(model).join(&download.root)
    }

    /// Whether `dir` (as returned above) holds a complete download of exactly this `download`.
    pub fn installed(dir: &Path, download: &Download) -> bool {
        let Some(slot) = dir.parent() else { return false };
        fs::read_to_string(slot.join(MARKER)).map(|s| s.trim() == download.sha256).unwrap_or(false)
    }

    /// Downloads and unpacks `download` so that `dir` (as returned above) exists. Idempotent.
    pub fn fetch(&self, download: &Download, dir: &Path, progress: OnProgress) -> Result<(), Error> {
        if Self::installed(dir, download) {
            return Ok(());
        }
        let slot = dir.parent().ok_or("no parent directory")?.to_path_buf();
        let parent = slot.parent().ok_or("no parent directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let partial = slot.with_extension("partial");
        download_to(download, &partial, progress)?;
        let staging = slot.with_extension("unpacking");
        let _ = fs::remove_dir_all(&staging);
        let unpacked = unpack(&partial, &staging, &download.archive);
        let _ = fs::remove_file(&partial);
        unpacked?;
        if !staging.join(&download.root).is_dir() {
            let _ = fs::remove_dir_all(&staging);
            return Err(format!("the archive has no {}/ directory", download.root));
        }
        fs::write(staging.join(MARKER), &download.sha256).map_err(|e| e.to_string())?;
        let _ = fs::remove_dir_all(&slot);
        fs::rename(&staging, &slot).map_err(|e| format!("cannot move the download into place: {e}"))?;
        Ok(())
    }
}

fn download_to(download: &Download, path: &Path, progress: OnProgress) -> Result<(), Error> {
    if !download.url.starts_with("https://") {
        return Err(format!("refusing a download that is not https: {}", download.url));
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client.get(&download.url).send().map_err(|e| format!("{}: {e}", download.url))?;
    if !response.status().is_success() {
        return Err(format!("{}: HTTP {}", download.url, response.status()));
    }
    let expected = download.size;
    let mut file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let read = response.read(&mut buffer).map_err(|e| format!("download interrupted: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        done += read as u64;
        progress(done, expected);
    }
    file.flush().map_err(|e| e.to_string())?;
    let got = hex(&hasher.finalize());
    if got != download.sha256.to_ascii_lowercase() {
        let _ = fs::remove_file(path);
        return Err(format!("{}: SHA-256 {got}, the catalog says {}", download.url, download.sha256));
    }
    Ok(())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks `archive` into `into`. `tar`'s `unpack_in` refuses entries that would land outside `into`.
pub fn unpack(archive: &Path, into: &Path, kind: &str) -> Result<(), Error> {
    if kind != "tar.bz2" {
        return Err(format!("unsupported archive {kind}"));
    }
    fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let file = fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(file)));
    tar.set_preserve_permissions(false);
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue; // no links, devices or anything else a model needs
        }
        if !entry.unpack_in(into).map_err(|e| e.to_string())? {
            return Err(format!("archive entry outside its directory: {:?}", entry.path().ok()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tarball(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("a.tar.bz2");
        let encoder = bzip2::write::BzEncoder::new(fs::File::create(&path).unwrap(), bzip2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn unpacks_a_tar_bz2() {
        let temp = std::env::temp_dir().join(format!("sv-unpack-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        let archive = tarball(&temp, &[("model/a.onnx", b"abc"), ("model/sub/b.txt", b"hello")]);
        unpack(&archive, &temp.join("out"), "tar.bz2").unwrap();
        assert_eq!(fs::read(temp.join("out/model/sub/b.txt")).unwrap(), b"hello");
        assert!(unpack(&archive, &temp.join("x"), "zip").is_err());
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn installed_means_the_marker_names_this_hash() {
        let temp = std::env::temp_dir().join(format!("sv-marker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let store = Store::new(&temp);
        let download = Download {
            url: "https://example.com/m.tar.bz2".into(),
            sha256: "ab".repeat(32),
            size: 1,
            archive: "tar.bz2".into(),
            root: "m".into(),
        };
        let dir = store.model_dir("sherpa-onnx", "tiny", &download);
        assert!(!Store::installed(&dir, &download));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.parent().unwrap().join(MARKER), "cd".repeat(32)).unwrap();
        assert!(!Store::installed(&dir, &download), "another version");
        fs::write(dir.parent().unwrap().join(MARKER), "ab".repeat(32)).unwrap();
        assert!(Store::installed(&dir, &download));
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn refuses_plain_http() {
        let download = Download {
            url: "http://example.com/x".into(),
            sha256: "00".repeat(32),
            size: 0,
            archive: "tar.bz2".into(),
            root: "x".into(),
        };
        let mut progress = |_, _| {};
        assert!(download_to(&download, Path::new("/nonexistent"), &mut progress).unwrap_err().contains("https"));
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(hex(&[0xab, 0x01]), "ab01");
    }
}
