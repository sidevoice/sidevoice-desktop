//! Downloading an engine package or a model's files: streamed to disk, SHA-256 checked against the catalog,
//! unpacked into a temporary directory and moved into place only when whole. A download counts as installed only
//! when its `.sidevoice-complete` marker names the hash it came from **and** its root and every file the engine
//! needs from it are there; one that lost a file is not installed, and fetching it again repairs it. A fetch that
//! fails or is stopped (a cancelled install) removes what it had written: nothing partial stays on disk.

use crate::error::{self, Error};
use sha2::{Digest, Sha256};
use sidevoice_desktop_core::engines::Download;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub(crate) const MARKER: &str = ".sidevoice-complete";

/// Where downloads live: `<root>/engines/…` and `<root>/models/…` (the app's data directory).
#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
    /// Where a download's bytes come from: HTTPS in the app, served from memory in tests.
    pub(crate) open: Open,
}

/// Opens the bytes behind a download's https URL.
pub type Open = fn(&str) -> Result<Box<dyn Read + Send>, Error>;

/// Progress: bytes so far, bytes expected.
pub type OnProgress<'a> = &'a mut dyn FnMut(u64, u64);

/// Asked between chunks and archive entries: true stops the fetch with `install_cancelled`.
pub type Stop<'a> = &'a dyn Fn() -> bool;

/// `file` names something inside a download's root: relative, no `..`, nothing absolute.
fn inside(file: &str) -> bool {
    !file.is_empty() && Path::new(file).components().all(|c| matches!(c, Component::Normal(_)))
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into(), open: https }
    }

    /// The unpacked root of an engine package, e.g. `engines/sherpa-onnx-1.13.8-macos-aarch64/<root>`.
    pub fn engine_dir(&self, engine: &str, version: &str, os: &str, arch: &str, download: &Download) -> PathBuf {
        self.root.join("engines").join(format!("{engine}-{version}-{os}-{arch}")).join(&download.root)
    }

    /// The unpacked root of a model's files for one engine, e.g. `models/sherpa-onnx/whisper-tiny/<root>`.
    pub fn model_dir(&self, engine: &str, model: &str, download: &Download) -> PathBuf {
        self.root.join("models").join(engine).join(model).join(&download.root)
    }

    /// Whether `dir` (as returned above) holds a complete download of exactly this `download`, with every one of
    /// `files` (relative to `dir`) in it.
    pub fn installed(dir: &Path, download: &Download, files: &[String]) -> bool {
        let Some(slot) = dir.parent() else { return false };
        let marked = fs::read_to_string(slot.join(MARKER)).map(|s| s.trim() == download.sha256).unwrap_or(false);
        marked && dir.is_dir() && files.iter().all(|f| inside(f) && dir.join(f).exists())
    }

    /// Bytes a download (`dir` as returned above) takes on disk, everything it unpacked included.
    pub fn bytes_on_disk(dir: &Path) -> u64 {
        fn walk(path: &Path) -> u64 {
            let Ok(meta) = fs::symlink_metadata(path) else { return 0 };
            if !meta.is_dir() {
                return meta.len();
            }
            fs::read_dir(path).map(|entries| entries.flatten().map(|e| walk(&e.path())).sum()).unwrap_or(0)
        }
        dir.parent().map(walk).unwrap_or(0)
    }

    /// Downloads and unpacks `download` so that `dir` (as returned above) holds it with all of `files`. Idempotent:
    /// nothing to do when it is installed. A slot that is marked but incomplete loses its marker first, so it is
    /// never taken for complete again. On failure, or when `stop` says so, the slot, the partial download and the
    /// unpacking directory are removed: what is left on disk is what was whole before.
    pub fn fetch(
        &self,
        download: &Download,
        dir: &Path,
        files: &[String],
        progress: OnProgress,
        stop: Stop,
    ) -> Result<(), Error> {
        if Self::installed(dir, download, files) {
            return Ok(());
        }
        let slot = dir.parent().ok_or_else(|| error::internal("a download directory without a parent"))?.to_path_buf();
        let parent = slot.parent().ok_or_else(|| error::internal("a download directory without a parent"))?;
        let _ = fs::remove_file(slot.join(MARKER));
        fs::create_dir_all(parent).map_err(error::install_failed)?;
        let (partial, staging) = (slot.with_extension("partial"), slot.with_extension("unpacking"));
        let fetched = self.fetch_into(download, &partial, &staging, files, progress, stop);
        let _ = fs::remove_file(&partial);
        if let Err(e) = fetched {
            let _ = fs::remove_dir_all(&staging);
            let _ = fs::remove_dir_all(&slot); // unmarked: an incomplete download nothing can use
            let _ = fs::remove_dir(parent); // only when nothing else is in it
            return Err(e);
        }
        let _ = fs::remove_dir_all(&slot);
        fs::rename(&staging, &slot).map_err(error::install_failed)?;
        Ok(())
    }

    /// Downloads into `partial` and unpacks it into `staging`, marked complete, ready to move into place.
    fn fetch_into(
        &self,
        download: &Download,
        partial: &Path,
        staging: &Path,
        files: &[String],
        progress: OnProgress,
        stop: Stop,
    ) -> Result<(), Error> {
        download_to(download, self.open, partial, progress, stop)?;
        let _ = fs::remove_dir_all(staging);
        unpack(partial, staging, &download.archive, stop)?;
        let root = staging.join(&download.root);
        let missing = if root.is_dir() {
            files.iter().find(|f| !inside(f) || !root.join(f).exists()).map(|f| format!("{}/{f}", download.root))
        } else {
            Some(format!("{}/", download.root))
        };
        if let Some(missing) = missing {
            return Err(error::install_failed(format!("the archive has no {missing}")));
        }
        if stop() {
            return Err(error::install_cancelled());
        }
        fs::write(staging.join(MARKER), &download.sha256).map_err(error::install_failed)
    }
}

/// The app's source of downloads: the URL over HTTPS.
fn https(url: &str) -> Result<Box<dyn Read + Send>, Error> {
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| error::download_failed(url, e))?;
    let response = client.get(url).send().map_err(|e| error::download_failed(url, e))?;
    if !response.status().is_success() {
        return Err(error::download_failed(url, format!("HTTP {}", response.status())));
    }
    Ok(Box::new(response))
}

fn download_to(download: &Download, open: Open, path: &Path, progress: OnProgress, stop: Stop) -> Result<(), Error> {
    if !download.url.starts_with("https://") {
        return Err(error::download_refused(&download.url));
    }
    let mut response = open(&download.url)?;
    let expected = download.size;
    let mut file = fs::File::create(path).map_err(error::install_failed)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let read = response.read(&mut buffer).map_err(|e| error::download_failed(&download.url, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(error::install_failed)?;
        done += read as u64;
        progress(done, expected);
        if stop() {
            return Err(error::install_cancelled());
        }
    }
    file.flush().map_err(error::install_failed)?;
    let got = hex(&hasher.finalize());
    if got != download.sha256.to_ascii_lowercase() {
        return Err(error::download_corrupt(&download.url));
    }
    Ok(())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks `archive` into `into`, asking `stop` before each entry. `tar`'s `unpack_in` refuses entries that would
/// land outside `into`.
pub fn unpack(archive: &Path, into: &Path, kind: &str, stop: Stop) -> Result<(), Error> {
    if kind != "tar.bz2" {
        return Err(error::install_failed(format!("unsupported archive {kind}")));
    }
    fs::create_dir_all(into).map_err(error::install_failed)?;
    let file = fs::File::open(archive).map_err(error::install_failed)?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(std::io::BufReader::new(file)));
    tar.set_preserve_permissions(false);
    for entry in tar.entries().map_err(error::install_failed)? {
        if stop() {
            return Err(error::install_cancelled());
        }
        let mut entry = entry.map_err(error::install_failed)?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue; // no links, devices or anything else a model needs
        }
        if !entry.unpack_in(into).map_err(error::install_failed)? {
            return Err(error::install_failed(format!("archive entry outside its directory: {:?}", entry.path().ok())));
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
        unpack(&archive, &temp.join("out"), "tar.bz2", &|| false).unwrap();
        assert_eq!(fs::read(temp.join("out/model/sub/b.txt")).unwrap(), b"hello");
        assert!(unpack(&archive, &temp.join("x"), "zip", &|| false).is_err());
        let stopped = unpack(&archive, &temp.join("y"), "tar.bz2", &|| true).unwrap_err();
        assert_eq!(stopped.key, "install_cancelled");
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn installed_means_the_marker_names_this_hash_and_the_files_are_there() {
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
        let files = vec!["model.onnx".to_string(), "espeak-ng-data/phontab".to_string()];
        let dir = store.model_dir("sherpa-onnx", "tiny", &download);
        assert!(!Store::installed(&dir, &download, &files));
        fs::create_dir_all(dir.join("espeak-ng-data")).unwrap();
        fs::write(dir.join("model.onnx"), b"m").unwrap();
        fs::write(dir.join("espeak-ng-data/phontab"), b"p").unwrap();
        fs::write(dir.parent().unwrap().join(MARKER), "cd".repeat(32)).unwrap();
        assert!(!Store::installed(&dir, &download, &files), "another version");
        fs::write(dir.parent().unwrap().join(MARKER), "ab".repeat(32)).unwrap();
        assert!(Store::installed(&dir, &download, &files));

        fs::remove_file(dir.join("espeak-ng-data/phontab")).unwrap();
        assert!(!Store::installed(&dir, &download, &files), "a file the engine needs is gone");
        assert!(Store::installed(&dir, &download, &files[..1]));
        assert!(!Store::installed(&dir, &download, &["../m/model.onnx".to_string()]), "outside the root");
        fs::remove_dir_all(&dir).unwrap();
        assert!(!Store::installed(&dir, &download, &[]), "the marker alone is not a download");
        fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn fetching_an_incomplete_slot_unmarks_it_before_downloading_again() {
        let temp = std::env::temp_dir().join(format!("sv-repair-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let store = Store::new(&temp);
        let download = Download {
            url: "https://127.0.0.1:9/m.tar.bz2".into(), // nothing listens: the download fails, fast
            sha256: "ab".repeat(32),
            size: 1,
            archive: "tar.bz2".into(),
            root: "m".into(),
        };
        let dir = store.model_dir("sherpa-onnx", "tiny", &download);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.parent().unwrap().join(MARKER), "ab".repeat(32)).unwrap();
        let files = vec!["model.onnx".to_string()];
        let mut progress = |_, _| {};
        let failed = store.fetch(&download, &dir, &files, &mut progress, &|| false).unwrap_err();
        assert_eq!(failed.key, "download_failed", "it tried to download it again");
        assert!(!dir.parent().unwrap().join(MARKER).exists(), "and the slot is no longer marked complete");
        assert!(!dir.parent().unwrap().exists(), "nor kept: an unmarked download nothing can use");
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
        let refused = download_to(&download, https, Path::new("/nonexistent"), &mut progress, &|| false);
        assert_eq!(refused.unwrap_err().key, "download_refused");
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(hex(&[0xab, 0x01]), "ab01");
    }
}
