//! Native file-manager selection, provided by Tauri's official opener plugin on every desktop OS.
//! The local-host command is still gated off on Windows until its separate platform migration is complete.
#![cfg_attr(target_os = "windows", allow(dead_code))]

use std::path::PathBuf;
use tauri::{AppHandle, Runtime};
use tauri_plugin_opener::OpenerExt;

pub trait Reveal {
    /// Reveal existing files in their containing directory using the native file manager. Preserve path values
    /// (including non-UTF-8 names); propagate plugin/IO failures to the caller's localized refusal mapping.
    /// The opener plugin must be registered on `app`. No page-side opener permissions are needed.
    fn files<R: Runtime>(app: &AppHandle<R>, paths: &[PathBuf]) -> Result<(), tauri_plugin_opener::Error> {
        app.opener().reveal_items_in_dir(paths)
    }
}

pub struct NativeReveal;
impl Reveal for NativeReveal {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_propagates_the_plugin_io_error_without_opening_a_window() {
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        // Spaces and shell metacharacters remain path characters; no command string is constructed.
        let missing = dir.path().join("missing log ; $.log");
        let error = NativeReveal::files(app.handle(), &[missing]).unwrap_err();
        assert!(matches!(error, tauri_plugin_opener::Error::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound));
    }
}
