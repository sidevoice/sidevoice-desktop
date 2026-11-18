//! The shared crate's error codes for the log (the Sidevoice logging guideline, section 6): one enum, a stable
//! `shared.<kebab-name>` string per variant, logged as `error.type`. `Display` is for people and callers and is never logged.

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SharedError {
    #[error("SIDEVOICE_LOG is not a valid filter")]
    InvalidLogFilter,
    #[error("the native engine did not start")]
    NativeEngineFailed,
    #[error("voice models were still held at exit")]
    VoiceModelsHeld,
    #[error("a crash report could not be sent")]
    CrashReportNotSent,
    #[error("the crash records could not be read or deleted")]
    CrashRecordsUnavailable,
    #[error("the crash report settings could not be kept")]
    CrashSettingsNotKept,
    #[error("the crash report settings could not be read")]
    CrashSettingsUnreadable,
    #[error("a step of the crash reporter failed and was contained")]
    CrashReporterFailed,
}

impl SharedError {
    /// Every variant: what [`SharedError::codes`] lists and the tests that hold the codes unique and well formed.
    pub const ALL: [SharedError; 8] = [
        SharedError::InvalidLogFilter,
        SharedError::NativeEngineFailed,
        SharedError::VoiceModelsHeld,
        SharedError::CrashReportNotSent,
        SharedError::CrashRecordsUnavailable,
        SharedError::CrashSettingsNotKept,
        SharedError::CrashSettingsUnreadable,
        SharedError::CrashReporterFailed,
    ];

    /// Every code this enum can give: the closed list of what the shared crate calls an error. A handled error is reported only
    /// under an `error.type` that an app's list holds (`crash::report_handled_errors`).
    pub fn codes() -> impl Iterator<Item = &'static str> {
        Self::ALL.iter().map(SharedError::code)
    }

    /// The stable code, `error.type` in the log.
    pub fn code(&self) -> &'static str {
        match self {
            SharedError::InvalidLogFilter => "shared.invalid-log-filter",
            SharedError::NativeEngineFailed => "shared.native-engine-failed",
            SharedError::VoiceModelsHeld => "shared.voice-models-held",
            SharedError::CrashReportNotSent => "shared.crash-report-not-sent",
            SharedError::CrashRecordsUnavailable => "shared.crash-records-unavailable",
            SharedError::CrashSettingsNotKept => "shared.crash-settings-not-kept",
            SharedError::CrashSettingsUnreadable => "shared.crash-settings-unreadable",
            SharedError::CrashReporterFailed => "shared.crash-reporter-failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_shaped_like_codes() {
        let mut seen = std::collections::HashSet::new();
        for error in SharedError::ALL {
            let code = error.code();
            assert!(seen.insert(code), "{code} twice");
            let name = code.strip_prefix("shared.").expect("the component prefix");
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
            .split("pub enum SharedError {")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("the enum is in this file");
        let declared =
            body.lines().map(str::trim).filter(|line| line.starts_with(|c: char| c.is_ascii_uppercase())).count();
        assert!(declared > 5, "the enum was not read");
        assert_eq!(SharedError::ALL.len(), declared, "a variant is not in ALL");
        assert_eq!(SharedError::codes().count(), declared);
    }
}
