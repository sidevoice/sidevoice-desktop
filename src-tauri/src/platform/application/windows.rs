//! Tauri's defaults already provide the application lifecycle on this host.

use super::Os;

pub(super) struct Native;

impl Os for Native {}

#[cfg(test)]
mod tests {
    /// The manifest (packaging/windows/app.manifest) makes UTF-8 the process's ANSI code page, which the native engine
    /// needs: espeak-ng opens its data directory through a narrow path and aborts the process on one with non-ASCII
    /// characters under any other code page (sidevoice-engine#130). Tests run under the same manifest as the app.
    #[test]
    fn the_process_ansi_code_page_is_utf8() {
        // SAFETY: `GetACP` takes no arguments and only reads the process's code page.
        assert_eq!(unsafe { windows_sys::Win32::Globalization::GetACP() }, 65001);
    }
}
