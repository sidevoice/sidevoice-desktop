//! Version and compatibility decisions for the explicit R4 update action.
//!
//! Missing R4 metadata is `unknown`: an older `npx` connector remains usable, but the app never guesses that an update
//! is safe. Ordering mirrors the connector's `release.decide`: connector semver, nightly build sequence, then the SEA
//! format tie-breaker within the same release ordering. The desktop app's version is never involved.

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
            if nightly_candidate && candidate > current {
                return UpdateStatus::Available;
            }
            if installed_channel == "source" && candidate_channel != "source" {
                // Connector release.decide replaces a source checkout with a packaged channel, even at the same
                // connector version. The installed source identity is not the bundled SEA's identity.
                return UpdateStatus::Available;
            }
            if !same_channel {
                // The connector treats cross-channel same-version selections as a no-op unless the candidate nightly
                // sequence above is newer. A lower sequence does not mean this cross-channel artifact should be used.
                return UpdateStatus::Current;
            }
            if nightly_candidate && candidate < current {
                return UpdateStatus::NewerInstalled;
            }
            if nightly_candidate && candidate != current {
                return UpdateStatus::Unknown;
            }
            // The candidate is always a verified SEA. R1 selected records may omit `format`; the connector treats
            // that as ESM. A same-ordering ESM → SEA change is an upgrade even when release build_seq differs.
            match installed.format.as_deref().unwrap_or("esm") {
                "esm" => UpdateStatus::Available,
                "sea" => UpdateStatus::Current,
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
            format: Some("sea".into()),
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
            UpdateStatus::Available,
            "a newer same-version nightly build replaces an older release regardless of format"
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "release", 42, Some(1), Some(1)))),
            UpdateStatus::Current,
            "a same-sequence cross-channel decision is a connector no-op"
        );
        assert_eq!(
            update_status(Some(&pin), Some(&installed("1.2.3", "release", 43, Some(1), Some(1)))),
            UpdateStatus::Current,
            "a lower same-version nightly candidate is a cross-channel no-op"
        );
        let mut release_pin = pin.clone();
        release_pin.channel = Some("release".into());
        assert_eq!(
            update_status(Some(&release_pin), Some(&installed("1.2.3", "source", 0, Some(1), Some(1)))),
            UpdateStatus::Available,
            "the connector upgrades a source checkout to a release candidate at the same version"
        );
    }

    #[test]
    fn release_build_sequence_is_ignored_but_same_version_esm_upgrades_to_sea() {
        let mut pin = crate::pin::test_support::fixture_pin();
        pin.channel = Some("release".into());
        pin.build_seq = Some(99);
        let mut installed = installed("1.2.3", "release", 1, Some(1), Some(1));
        installed.format = Some("esm".into());
        assert_eq!(update_status(Some(&pin), Some(&installed)), UpdateStatus::Available);
        installed.format = Some("sea".into());
        installed.build_seq = Some(100);
        assert_eq!(
            update_status(Some(&pin), Some(&installed)),
            UpdateStatus::Current,
            "release build_seq cannot make a same-version release look newer"
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
