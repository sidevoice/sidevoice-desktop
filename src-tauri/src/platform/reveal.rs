//! Native file-manager selection, provided by Tauri's official opener plugin on every desktop OS.
//! The local-host command is still gated off on Windows until its separate platform migration is complete.
#![cfg_attr(target_os = "windows", allow(dead_code))]

use std::path::PathBuf;
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;

pub trait Reveal {
    /// Reveal existing files in their containing directory using the native file manager. Resolve all paths
    /// before revealing anything; reject non-UTF-8 targets with InvalidInput rather than select a lossy name.
    /// Propagate plugin/IO failures to the caller's localized refusal mapping.
    /// The opener plugin must be registered on `app`. No page-side opener permissions are needed.
    fn files<R: Runtime>(app: &AppHandle<R>, paths: &[PathBuf]) -> Result<(), tauri_plugin_opener::Error> {
        let paths = paths.iter().map(std::fs::canonicalize).collect::<std::io::Result<Vec<_>>>()?;
        // Opener 2.7.0 converts canonical paths to strings lossily on macOS. Validate targets too, because
        // a UTF-8 symlink can resolve to a non-UTF-8 path. Keep one explicit contract on every host.
        if paths.iter().any(|path| path.to_str().is_none()) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "non-UTF-8 reveal path").into());
        }
        app.opener().reveal_items_in_dir(paths)
    }
}

pub struct NativeReveal;
impl Reveal for NativeReveal {}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap()
    }

    #[test]
    fn a_missing_file_propagates_the_plugin_io_error_without_opening_a_window() {
        let app = app();
        let dir = tempfile::tempdir().unwrap();
        // Spaces and shell metacharacters remain path characters; no command string is constructed.
        let missing = dir.path().join("missing log ; $.log");
        let error = NativeReveal::files(app.handle(), &[missing]).unwrap_err();
        assert!(matches!(error, tauri_plugin_opener::Error::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound));
    }

    // Arbitrary non-UTF-8 filesystem bytes can only be constructed on Unix. The interface and validation
    // are common to every OS; Windows compiles the same code and keeps the missing-file contract test.
    #[cfg(unix)]
    #[test]
    fn non_utf8_targets_are_refused_even_when_a_lossy_twin_exists() {
        use std::ffi::OsString;
        use std::os::unix::{ffi::OsStringExt, fs::symlink};

        let app = app();
        let dir = tempfile::tempdir().unwrap();
        let raw_dir = dir.path().join(OsString::from_vec(b"logs-\xff".to_vec()));
        let lossy_dir = dir.path().join(raw_dir.file_name().unwrap().to_string_lossy().as_ref());
        std::fs::create_dir(&raw_dir).unwrap();
        std::fs::create_dir(&lossy_dir).unwrap();
        let raw_log = raw_dir.join("app.log");
        std::fs::write(&raw_log, "original").unwrap();
        std::fs::write(lossy_dir.join("app.log"), "different file").unwrap();
        let link = dir.path().join("utf8-link.log");
        symlink(&raw_log, &link).unwrap();
        assert!(link.to_str().is_some());

        for path in [raw_log, link] {
            let error = NativeReveal::files(app.handle(), &[path]).unwrap_err();
            assert!(
                matches!(error, tauri_plugin_opener::Error::Io(ref io) if io.kind() == std::io::ErrorKind::InvalidInput)
            );
        }
    }
}
