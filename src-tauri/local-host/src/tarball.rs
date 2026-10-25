// Existing OS debt is capped by platform-debt.txt.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

//! The platform package's tarball, unpacked by hand into the install's private directory (docs/LOCAL_HOST.md →
//! Install): a gzipped tar whose every entry is a plain file or directory below `package/`.
//!
//! Refused, with `install.authenticity` and the check the page names: a path that is absolute, has an empty, `.` or
//! `..` segment, a backslash or a control character, or is not below `package/` (`archive-path`); a symbolic or hard
//! link (`archive-link`); a device, a FIFO, a sparse file or anything else that is not a file or a directory
//! (`archive-type`, also for an archive that does not decode); more than [`Limits`] allow (`archive-size`). Nothing is
//! taken from an entry but its path, its contents and whether it is executable: files are written 0600 (0700 when the
//! archive marks them executable, as `bin/sidevoice-connector`), directories 0700, each created anew — an entry that
//! is already there (twice in the archive, or a file where a directory is) is refused (`archive-path`). No link is
//! ever created, so a path built from checked segments below the directory stays below it.

use crate::install::{Failure, Step};
use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// What one archive may unpack to.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The files' bytes, all together.
    pub bytes: u64,
    /// Entries of any kind.
    pub entries: usize,
}

/// The real package unpacks to about 15 MB in about a hundred files.
pub const LIMITS: Limits = Limits { bytes: 1024 * 1024 * 1024, entries: 10_000 };

/// The top directory every entry of an npm tarball is below.
pub const ROOT: &str = "package";

const PATH_LIMIT: usize = 1024;
const DEPTH_LIMIT: usize = 32;

fn refused(check: &str, message: impl Into<String>) -> Failure {
    Failure::new("install.authenticity", Step::Staging, message).check(check)
}

fn written(e: io::Error, path: &Path) -> Failure {
    Failure::io(&e, Step::Staging, &format!("Could not write {}", path.display()))
}

/// An entry's path as checked segments, the first of them [`ROOT`].
fn segments(raw: &[u8]) -> Result<Vec<&str>, Failure> {
    let shown = String::from_utf8_lossy(raw);
    let bad = |why: &str| refused("archive-path", format!("The archive holds {shown:?}: {why}."));
    let text = std::str::from_utf8(raw).map_err(|_| bad("not UTF-8"))?;
    if text.is_empty() || text.len() > PATH_LIMIT {
        return Err(bad("an empty or overlong path"));
    }
    if text.chars().any(|c| c.is_control() || c == '\\') {
        return Err(bad("a control character or a backslash"));
    }
    if text.starts_with('/') {
        return Err(bad("an absolute path"));
    }
    // A directory may end with `/`; nothing else may be empty.
    let segments: Vec<&str> = text.strip_suffix('/').unwrap_or(text).split('/').collect();
    if segments.iter().any(|s| s.is_empty() || *s == "." || *s == "..") {
        return Err(bad("an empty, `.` or `..` segment"));
    }
    if segments[0] != ROOT || segments.len() > DEPTH_LIMIT {
        return Err(bad("not below package/"));
    }
    Ok(segments)
}

/// Creates `path` (0700) unless it is a directory already: one this unpacking made, since nothing else writes here.
fn directory(path: &Path) -> Result<(), Failure> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_dir() => Ok(()),
            _ => Err(refused(
                "archive-path",
                format!("The archive puts a directory where a file is: {}.", path.display()),
            )),
        },
        Err(e) => Err(written(e, path)),
    }
}

/// Unpacks the gzipped tar `archive` into `into` (an existing private directory): see the module's documentation.
/// `cancelled` is asked between entries and between chunks; when it says so, this stops with `install.cancelled`.
pub fn extract(archive: impl Read, into: &Path, limits: Limits, cancelled: &dyn Fn() -> bool) -> Result<(), Failure> {
    let undecodable = |e: io::Error| refused("archive-type", format!("The package does not unpack: {e}"));
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut entries_seen = 0usize;
    let mut bytes = 0u64;
    for entry in archive.entries().map_err(undecodable)? {
        if cancelled() {
            return Err(Failure::cancelled(Step::Staging));
        }
        let mut entry = entry.map_err(undecodable)?;
        entries_seen += 1;
        if entries_seen > limits.entries {
            return Err(refused("archive-size", format!("The package has more than {} entries.", limits.entries)));
        }
        let kind = entry.header().entry_type();
        // A pax global header describes the archive, not a file (pax per-file headers and GNU long names are applied
        // to the entry they precede by the reader).
        if kind.is_pax_global_extensions() {
            continue;
        }
        let raw = entry.path_bytes().into_owned();
        let segments = segments(&raw)?;
        let path: PathBuf = segments.iter().fold(into.to_path_buf(), |path, segment| path.join(segment));
        if kind.is_symlink() || kind.is_hard_link() {
            return Err(refused(
                "archive-link",
                format!("The archive holds a link: {}.", String::from_utf8_lossy(&raw)),
            ));
        }
        if kind.is_dir() {
            let mut at = into.to_path_buf();
            for segment in &segments {
                at.push(segment);
                directory(&at)?;
            }
            continue;
        }
        if !(kind.is_file() || kind.is_contiguous()) || segments.len() < 2 {
            return Err(refused(
                "archive-type",
                format!("The archive holds something that is not a file: {}.", String::from_utf8_lossy(&raw)),
            ));
        }
        let mut at = into.to_path_buf();
        for segment in &segments[..segments.len() - 1] {
            at.push(segment);
            directory(&at)?;
        }
        let executable = entry.header().mode().map(|mode| mode & 0o111 != 0).unwrap_or(false);
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if executable { 0o700 } else { 0o600 })
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(refused("archive-path", format!("The archive holds {} twice.", path.display())));
            }
            Err(e) => return Err(written(e, &path)),
        };
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            if cancelled() {
                return Err(Failure::cancelled(Step::Staging));
            }
            let n = match entry.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(undecodable(e)),
            };
            bytes += n as u64;
            if bytes > limits.bytes {
                return Err(refused(
                    "archive-size",
                    format!("The package unpacks to more than {} bytes.", limits.bytes),
                ));
            }
            file.write_all(&chunk[..n]).map_err(|e| written(e, &path))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sidevoice_desktop_test_support::fake_registry::{gzip, tar_entry, tarball};
    use std::os::unix::fs::PermissionsExt;

    fn unpack(bytes: &[u8]) -> (tempfile::TempDir, Result<(), Failure>) {
        let tmp = tempfile::tempdir().unwrap();
        let result = extract(bytes, tmp.path(), LIMITS, &|| false);
        (tmp, result)
    }

    fn refused_with(bytes: &[u8], check: &str) {
        let (tmp, result) = unpack(bytes);
        let failure = result.expect_err("refused");
        assert_eq!(failure.key, "install.authenticity", "{failure:?}");
        assert_eq!(failure.step, Some(Step::Staging));
        assert_eq!(failure.params, Some(json!({"check": check})), "{failure:?}");
        // Nothing was written outside the directory it was given.
        assert!(!tmp.path().parent().unwrap().join("evil").exists());
    }

    #[test]
    fn a_package_unpacks_with_its_executable_and_private_modes() {
        let archive = tarball(&[
            ("package/package.json", b"{}".as_slice(), 0o644),
            ("package/bin/sidevoice-connector", b"#!/bin/sh\n".as_slice(), 0o755),
            ("package/notices/licenses/a.txt", b"MIT".as_slice(), 0o644),
        ]);
        let (tmp, result) = unpack(&archive);
        result.unwrap();
        let root = tmp.path().join("package");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(std::fs::read(root.join("bin/sidevoice-connector")).unwrap(), b"#!/bin/sh\n");
        assert_eq!(mode(&root.join("bin/sidevoice-connector")), 0o700);
        assert_eq!(mode(&root.join("package.json")), 0o600);
        assert_eq!(mode(&root.join("notices/licenses")), 0o700);
        assert_eq!(std::fs::read(root.join("notices/licenses/a.txt")).unwrap(), b"MIT");
    }

    #[test]
    fn explicit_directories_are_fine() {
        let mut raw = Vec::new();
        raw.extend(tar_entry("package/", b"", tar::EntryType::Directory, 0o755, None));
        raw.extend(tar_entry("package/bin/", b"", tar::EntryType::Directory, 0o755, None));
        raw.extend(tar_entry("package/bin/x", b"x", tar::EntryType::Regular, 0o644, None));
        raw.extend([0u8; 1024]);
        let (tmp, result) = unpack(&gzip(&raw));
        result.unwrap();
        assert!(tmp.path().join("package/bin/x").is_file());
    }

    #[test]
    fn paths_that_leave_the_package_are_refused() {
        for path in [
            "package/../evil",
            "../evil",
            "/tmp/evil",
            "evil",
            "other/evil",
            "package//evil",
            "package/./evil",
            "./package/evil",
            "package\\..\\evil",
            "package/e\u{1b}vil",
            "",
        ] {
            let mut raw = tar_entry(path, b"x", tar::EntryType::Regular, 0o644, None);
            raw.extend([0u8; 1024]);
            refused_with(&gzip(&raw), "archive-path");
        }
    }

    #[test]
    fn a_long_name_cannot_smuggle_a_path_out() {
        // A GNU long name, and a pax `path`, are what the entry is called: checked as any other.
        let long = format!("package/{}/../../../evil", "a".repeat(120));
        let mut raw =
            tar_entry("././@LongLink", format!("{long}\0").as_bytes(), tar::EntryType::GNULongName, 0o644, None);
        raw.extend(tar_entry("package/short", b"x", tar::EntryType::Regular, 0o644, None));
        raw.extend([0u8; 1024]);
        refused_with(&gzip(&raw), "archive-path");

        let record = "path=package/../../evil\n";
        let pax = format!("{} {record}", record.len() + 3);
        let mut raw = tar_entry("PaxHeader", pax.as_bytes(), tar::EntryType::XHeader, 0o644, None);
        raw.extend(tar_entry("package/short", b"x", tar::EntryType::Regular, 0o644, None));
        raw.extend([0u8; 1024]);
        refused_with(&gzip(&raw), "archive-path");
    }

    #[test]
    fn links_are_refused_and_never_created() {
        for kind in [tar::EntryType::Symlink, tar::EntryType::Link] {
            let mut raw = tar_entry("package/bin/sidevoice-connector", b"", kind, 0o777, Some("/bin/sh"));
            raw.extend([0u8; 1024]);
            refused_with(&gzip(&raw), "archive-link");
        }
        // A link to a directory, then a file through it: the link is refused before anything goes through it.
        let mut raw = tar_entry("package/bin", b"", tar::EntryType::Symlink, 0o777, Some("../../.."));
        raw.extend(tar_entry("package/bin/evil", b"x", tar::EntryType::Regular, 0o644, None));
        raw.extend([0u8; 1024]);
        let (tmp, result) = unpack(&gzip(&raw));
        assert_eq!(result.unwrap_err().params, Some(json!({"check": "archive-link"})));
        assert!(std::fs::symlink_metadata(tmp.path().join("package/bin")).is_err(), "no link made");
    }

    #[test]
    fn devices_fifos_and_the_like_are_refused() {
        for kind in [tar::EntryType::Char, tar::EntryType::Block, tar::EntryType::Fifo, tar::EntryType::GNUSparse] {
            let mut raw = tar_entry("package/dev", b"", kind, 0o644, None);
            raw.extend([0u8; 1024]);
            refused_with(&gzip(&raw), "archive-type");
        }
        refused_with(b"not gzip at all", "archive-type");
        refused_with(
            &gzip(b"not a tar archive, but long enough to be read as a header block of one ............"),
            "archive-type",
        );
    }

    #[test]
    fn an_entry_twice_or_a_file_where_a_directory_is_is_refused() {
        let twice = tarball(&[("package/a", b"1".as_slice(), 0o644), ("package/a", b"2".as_slice(), 0o644)]);
        refused_with(&twice, "archive-path");
        let under_a_file = tarball(&[("package/a", b"1".as_slice(), 0o644), ("package/a/b", b"2".as_slice(), 0o644)]);
        let (_tmp, result) = unpack(&under_a_file);
        let failure = result.unwrap_err();
        assert!(failure.key == "install.authenticity" || failure.key == "install.failed", "{failure:?}");
    }

    #[test]
    fn more_than_the_limits_is_refused() {
        let archive = tarball(&[("package/a", vec![1u8; 4096].as_slice(), 0o644)]);
        let tmp = tempfile::tempdir().unwrap();
        let small = Limits { bytes: 1000, entries: 10 };
        let failure = extract(archive.as_slice(), tmp.path(), small, &|| false).unwrap_err();
        assert_eq!(failure.params, Some(json!({"check": "archive-size"})));
        let many = tarball(&[("package/a", b"".as_slice(), 0o644), ("package/b", b"".as_slice(), 0o644)]);
        let tmp = tempfile::tempdir().unwrap();
        let few = Limits { bytes: 1000, entries: 1 };
        let failure = extract(many.as_slice(), tmp.path(), few, &|| false).unwrap_err();
        assert_eq!(failure.params, Some(json!({"check": "archive-size"})));
    }

    #[test]
    fn a_cancel_stops_the_unpacking() {
        let archive = tarball(&[("package/a", b"1".as_slice(), 0o644)]);
        let tmp = tempfile::tempdir().unwrap();
        let failure = extract(archive.as_slice(), tmp.path(), LIMITS, &|| true).unwrap_err();
        assert_eq!((failure.key.as_str(), failure.step), ("install.cancelled", Some(Step::Staging)));
    }
}
