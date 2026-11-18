//! The local host's error codes for the log (the Sidevoice logging guideline, section 6): one enum for the crate, a stable
//! `local-host.<kebab-name>` string per variant, logged as `error.type`. `Display` is for people and callers and is never
//! logged: an error's text can carry a path or a foreign chain.

use crate::core_socket::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LocalHostError {
    #[error("the core is not reachable")]
    CoreAbsent,
    #[error("the core's directory or peer failed a check")]
    CoreUnsafe,
    #[error("the core did not answer as its contract says")]
    CoreFailed,
    #[error("the core did not answer in time")]
    CoreNoAnswer,
    #[error("the core refused the stored token")]
    TokenRefused,
    #[error("the pairing could not be stored")]
    PairingNotStored,
    #[error("this app is not paired with the core")]
    NotPaired,
    #[error("the service's newest version could not be checked")]
    ServiceCheckFailed,
    #[error("the service could not be updated")]
    ServiceUpdateFailed,
    #[error("the service install failed")]
    InstallFailed,
    #[error("the service install could not start")]
    InstallNotStarted,
}

impl LocalHostError {
    /// Every variant: what [`LocalHostError::codes`] lists and the tests that hold the codes unique and well formed.
    pub const ALL: [LocalHostError; 11] = [
        LocalHostError::CoreAbsent,
        LocalHostError::CoreUnsafe,
        LocalHostError::CoreFailed,
        LocalHostError::CoreNoAnswer,
        LocalHostError::TokenRefused,
        LocalHostError::PairingNotStored,
        LocalHostError::NotPaired,
        LocalHostError::ServiceCheckFailed,
        LocalHostError::ServiceUpdateFailed,
        LocalHostError::InstallFailed,
        LocalHostError::InstallNotStarted,
    ];

    /// Every code this enum can give: the closed list of what the local host calls an error, which the app adds to the codes a
    /// handled error may be reported under.
    pub fn codes() -> impl Iterator<Item = &'static str> {
        Self::ALL.iter().map(LocalHostError::code)
    }

    /// The stable code, `error.type` in the log.
    pub fn code(&self) -> &'static str {
        match self {
            LocalHostError::CoreAbsent => "local-host.core-absent",
            LocalHostError::CoreUnsafe => "local-host.core-unsafe",
            LocalHostError::CoreFailed => "local-host.core-failed",
            LocalHostError::CoreNoAnswer => "local-host.core-no-answer",
            LocalHostError::TokenRefused => "local-host.token-refused",
            LocalHostError::PairingNotStored => "local-host.pairing-not-stored",
            LocalHostError::NotPaired => "local-host.not-paired",
            LocalHostError::ServiceCheckFailed => "local-host.service-check-failed",
            LocalHostError::ServiceUpdateFailed => "local-host.service-update-failed",
            LocalHostError::InstallFailed => "local-host.install-failed",
            LocalHostError::InstallNotStarted => "local-host.install-not-started",
        }
    }
}

impl From<&CoreError> for LocalHostError {
    fn from(error: &CoreError) -> Self {
        match error {
            CoreError::Absent => LocalHostError::CoreAbsent,
            CoreError::Unsafe(_) => LocalHostError::CoreUnsafe,
            CoreError::Failed(_) => LocalHostError::CoreFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_shaped_like_codes() {
        let mut seen = std::collections::HashSet::new();
        for error in LocalHostError::ALL {
            let code = error.code();
            assert!(seen.insert(code), "{code} twice");
            let name = code.strip_prefix("local-host.").expect("the component prefix");
            assert!(
                !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{code}"
            );
        }
        assert_eq!(seen.len(), LocalHostError::ALL.len());
    }

    /// `ALL` is written by hand beside the enum, and what telemetry may send is what it lists: a variant that is added and not
    /// listed would be logged and never reported. The enum's own source is read, so adding one fails here until it is listed
    /// (a variant listed twice fails the test above).
    #[test]
    fn the_list_of_variants_holds_every_variant_of_the_enum() {
        // A checkout on Windows has CRLF line ends.
        let source = include_str!("error.rs").replace("\r\n", "\n");
        let body = source
            .split("pub enum LocalHostError {")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("the enum is in this file");
        let declared =
            body.lines().map(str::trim).filter(|line| line.starts_with(|c: char| c.is_ascii_uppercase())).count();
        assert!(declared > 5, "the enum was not read");
        assert_eq!(LocalHostError::ALL.len(), declared, "a variant is not in ALL");
        assert_eq!(LocalHostError::codes().count(), declared);
    }

    #[test]
    fn a_core_error_maps_to_its_own_code() {
        assert_eq!(LocalHostError::from(&CoreError::Absent).code(), "local-host.core-absent");
    }
}
