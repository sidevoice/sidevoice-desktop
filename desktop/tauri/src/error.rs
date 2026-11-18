//! The desktop app's error codes for the log (the Sidevoice logging guideline, section 6): one enum, a stable
//! `desktop.<kebab-name>` string per variant, logged as `error.type`. `Display` is for people and callers and is never logged.

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DesktopError {
    #[error("the updater did not start")]
    UpdaterStart,
    #[error("the update could not be downloaded")]
    UpdateDownloadFailed,
    #[error("the update could not be installed")]
    UpdateInstallFailed,
    #[error("the local host's update could not start")]
    LocalHostUpdateNotStarted,
    #[error("the local host's background updates could not start")]
    LocalHostUpdatesNotStarted,
    #[error("the crash report switch could not be copied to the core on this computer")]
    CrashSwitchNotCopied,
    #[error("the logs could not be shown")]
    RevealFailed,
    #[error("the webview's diagnostic switches could not be applied")]
    WebviewSwitchesFailed,
    #[error("the tray could not be created")]
    TrayUnavailable,
    #[error("the global shortcut plugin did not start")]
    ShortcutPluginFailed,
    #[error("a kept shortcut cannot be registered")]
    ShortcutUnregistrable,
    #[error("the carry-over from the previous identifier was not looked at")]
    LegacyNotLooked,
    #[error("the carry-over's journal could not be read")]
    LegacyJournalUnreadable,
    #[error("the carry-over's journal could not be written")]
    LegacyJournalUnwritable,
    #[error("an old folder could not be copied")]
    LegacyFolderFailed,
    #[error("the old provider keys could not be listed")]
    LegacyKeysNotListed,
    #[error("an old provider key could not be copied")]
    LegacyKeyFailed,
}

impl DesktopError {
    /// Every variant: what [`DesktopError::codes`] lists and the tests that hold the codes unique and well formed.
    pub const ALL: [DesktopError; 17] = [
        DesktopError::UpdaterStart,
        DesktopError::UpdateDownloadFailed,
        DesktopError::UpdateInstallFailed,
        DesktopError::LocalHostUpdateNotStarted,
        DesktopError::LocalHostUpdatesNotStarted,
        DesktopError::CrashSwitchNotCopied,
        DesktopError::RevealFailed,
        DesktopError::WebviewSwitchesFailed,
        DesktopError::TrayUnavailable,
        DesktopError::ShortcutPluginFailed,
        DesktopError::ShortcutUnregistrable,
        DesktopError::LegacyNotLooked,
        DesktopError::LegacyJournalUnreadable,
        DesktopError::LegacyJournalUnwritable,
        DesktopError::LegacyFolderFailed,
        DesktopError::LegacyKeysNotListed,
        DesktopError::LegacyKeyFailed,
    ];

    /// Every code this enum can give: the closed list of what the desktop app calls an error, which the app adds to the codes a
    /// handled error may be reported under (`crash_reports::codes`).
    pub fn codes() -> impl Iterator<Item = &'static str> {
        Self::ALL.iter().map(DesktopError::code)
    }

    /// The stable code, `error.type` in the log.
    pub fn code(&self) -> &'static str {
        match self {
            DesktopError::UpdaterStart => "desktop.updater-start",
            DesktopError::UpdateDownloadFailed => "desktop.update-download-failed",
            DesktopError::UpdateInstallFailed => "desktop.update-install-failed",
            DesktopError::LocalHostUpdateNotStarted => "desktop.local-host-update-not-started",
            DesktopError::LocalHostUpdatesNotStarted => "desktop.local-host-updates-not-started",
            DesktopError::CrashSwitchNotCopied => "desktop.crash-switch-not-copied",
            DesktopError::RevealFailed => "desktop.reveal-failed",
            DesktopError::WebviewSwitchesFailed => "desktop.webview-switches-failed",
            DesktopError::TrayUnavailable => "desktop.tray-unavailable",
            DesktopError::ShortcutPluginFailed => "desktop.shortcut-plugin-failed",
            DesktopError::ShortcutUnregistrable => "desktop.shortcut-unregistrable",
            DesktopError::LegacyNotLooked => "desktop.legacy-not-looked",
            DesktopError::LegacyJournalUnreadable => "desktop.legacy-journal-unreadable",
            DesktopError::LegacyJournalUnwritable => "desktop.legacy-journal-unwritable",
            DesktopError::LegacyFolderFailed => "desktop.legacy-folder-failed",
            DesktopError::LegacyKeysNotListed => "desktop.legacy-keys-not-listed",
            DesktopError::LegacyKeyFailed => "desktop.legacy-key-failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_shaped_like_codes() {
        let mut seen = std::collections::HashSet::new();
        for error in DesktopError::ALL {
            let code = error.code();
            assert!(seen.insert(code), "{code} twice");
            let name = code.strip_prefix("desktop.").expect("the component prefix");
            assert!(
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{code}"
            );
        }
    }

    /// `ALL` is written by hand beside the enum, and what telemetry may send is what it lists: a variant that is added and not
    /// listed would be logged and never reported. The enum's own source is read, so adding one fails here until it is listed
    /// (a variant listed twice fails the test above).
    #[test]
    fn the_list_of_variants_holds_every_variant_of_the_enum() {
        // A checkout on Windows has CRLF line ends.
        let source = include_str!("error.rs").replace("\r\n", "\n");
        let body = source
            .split("pub enum DesktopError {")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("the enum is in this file");
        let declared =
            body.lines().map(str::trim).filter(|line| line.starts_with(|c: char| c.is_ascii_uppercase())).count();
        assert!(declared > 5, "the enum was not read");
        assert_eq!(DesktopError::ALL.len(), declared, "a variant is not in ALL");
        assert_eq!(DesktopError::codes().count(), declared);
    }
}
