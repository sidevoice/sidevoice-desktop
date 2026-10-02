//! Version and compatibility decisions for the explicit R4 update action.
//!
//! Missing R4 metadata is `unknown`: an older `npx` connector remains usable, but the app never guesses that an update
//! is safe. All ordering uses connector versions and nightly build sequence, never the desktop app's version.

use crate::pin::{ConnectorPin, InstalledBuild};
use semver::Version;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateStatus {
    Available,
    Current,
    NewerInstalled,
    Incompatible,
    Unknown,
}

pub fn update_status(pin: Option<&ConnectorPin>, installed: Option<&InstalledBuild>) -> UpdateStatus {
    let (Some(pin), Some(installed)) = (pin, installed) else {
        return UpdateStatus::Unknown;
    };
    if pin.validate_ready().is_err() {
        return UpdateStatus::Unknown;
    }
    let Some(installed_version) = installed.connector_version.as_deref().and_then(|v| Version::parse(v).ok()) else {
        return UpdateStatus::Unknown;
    };
    let Some(candidate_version) = pin.connector_version.as_deref().and_then(|v| Version::parse(v).ok()) else {
        return UpdateStatus::Unknown;
    };
    let Some(candidate_api) = pin.core_api else { return UpdateStatus::Unknown };
    let Some(installed_api) = installed.core_api else { return UpdateStatus::Unknown };
    if !crate::state::API.contains(&candidate_api) || !crate::state::API.contains(&installed_api) {
        return UpdateStatus::Incompatible;
    }
    let Some(candidate_core) = pin.core_version.as_deref().and_then(|v| Version::parse(v).ok()) else {
        return UpdateStatus::Unknown;
    };
    let Some(installed_core) = installed.core_version.as_deref().and_then(|v| Version::parse(v).ok()) else {
        return UpdateStatus::Unknown;
    };
    if installed_core > candidate_core {
        return UpdateStatus::NewerInstalled;
    }
    let (Some(link_min), Some(link_max), Some(candidate_link)) = (pin.link_min, pin.link_max, pin.core_link) else {
        return UpdateStatus::Unknown;
    };
    // R1's selected release and health contracts do not expose the installed core link id. Verify the candidate link
    // against its pinned connector range, and reject an installed link when an R4 record provides one.
    if !(link_min..=link_max).contains(&candidate_link)
        || installed.core_link.is_some_and(|link| !(link_min..=link_max).contains(&link))
    {
        return UpdateStatus::Incompatible;
    }
    match candidate_version.cmp(&installed_version) {
        std::cmp::Ordering::Greater => UpdateStatus::Available,
        std::cmp::Ordering::Less => UpdateStatus::NewerInstalled,
        std::cmp::Ordering::Equal => {
            let (Some(candidate_channel), Some(installed_channel), Some(candidate), Some(current)) =
                (pin.channel.as_deref(), installed.channel.as_deref(), pin.build_seq, installed.build_seq)
            else {
                return UpdateStatus::Unknown;
            };
            let nightly_candidate = candidate_channel == "nightly";
            let same_channel = candidate_channel == installed_channel;
            match candidate.cmp(&current) {
                std::cmp::Ordering::Greater if nightly_candidate || same_channel => UpdateStatus::Available,
                std::cmp::Ordering::Less if nightly_candidate || same_channel => UpdateStatus::NewerInstalled,
                std::cmp::Ordering::Equal if same_channel => UpdateStatus::Current,
                // A same-version cross-channel change is not implied by semver. Do not guess that it is safe.
                _ => UpdateStatus::Unknown,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(version: &str, channel: &str, build_seq: u64, api: Option<i64>, link: Option<i64>) -> InstalledBuild {
        InstalledBuild {
            connector_version: Some(version.into()),
            connector_sha: Some("a".repeat(64)),
            core_version: Some("0.1.0".into()),
            core_build: Some("0.8.0-macos-aarch64-fixture".into()),
            core_manifest_sha256: Some("b".repeat(64)),
            channel: Some(channel.into()),
            build_seq: Some(build_seq),
            core_api: api,
            core_link: link,
        }
    }

    #[test]
    fn version_ordering_covers_noop_newer_installed_and_nightly_builds() {
        let pin = crate::pin::test_support::fixture_pin();
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.2", "release", 2, Some(1), Some(1)))),
            UpdateStatus::Available
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.3.0", "release", 3, Some(1), Some(1)))),
            UpdateStatus::NewerInstalled
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "nightly", 41, Some(1), Some(1)))),
            UpdateStatus::Available
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "nightly", 42, Some(1), Some(1)))),
            UpdateStatus::Current
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "nightly", 43, Some(1), Some(1)))),
            UpdateStatus::NewerInstalled
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "release", 41, Some(1), Some(1)))),
            UpdateStatus::Available
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "release", 42, Some(1), Some(1)))),
            UpdateStatus::Unknown
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "release", 43, Some(1), Some(1)))),
            UpdateStatus::NewerInstalled
        );
    }

    #[test]
    fn a_newer_installed_core_is_never_downgraded_by_a_connector_update() {
        let pin = crate::pin::test_support::fixture_pin();
        let mut installed = installed("1.2.2", "release", 41, Some(1), Some(1));
        installed.core_version = Some("0.2.0".into());
        assert_eq!(update_status(Some(&pin), Some(&installed)), UpdateStatus::NewerInstalled);
    }

    #[test]
    fn unknown_or_incompatible_metadata_never_offers_an_update() {
        let pin = crate::pin::test_support::fixture_pin();
        assert_eq!(
            update_status(None, Some(&installed("1.0.0", "release", 1, Some(1), Some(1)))),
            UpdateStatus::Unknown
        );
        assert_eq!(update_status(Some(&pin), None), UpdateStatus::Unknown);
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.0.0", "release", 1, Some(7), Some(1)))),
            UpdateStatus::Incompatible
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.0.0", "release", 1, Some(1), Some(9)))),
            UpdateStatus::Incompatible
        );
    }
}
