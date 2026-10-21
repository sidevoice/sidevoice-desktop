//! `host.build` (docs/BRIDGE.md → "The build"): what this build is, as `cargo xtask` told the app's build.rs
//! (`SIDEVOICE_BUILD_CHANNEL`, `_COMMIT`, `_DATE`; RELEASING.md → "Channels").

use serde::Serialize;

/// `{channel, version, commit?, date?}`: `release`, `nightly` or `dev` (build.rs refuses any other); a nightly carries
/// its own version (`X.Y.Z-nightly.RUN`, xtask) with its short commit and that commit's date (`YYYY-MM-DD`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    pub channel: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

impl BuildInfo {
    /// An empty commit or date is none.
    pub fn new(channel: &str, version: &str, commit: &str, date: &str) -> BuildInfo {
        let given = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        BuildInfo { channel: channel.to_owned(), version: version.to_owned(), commit: given(commit), date: given(date) }
    }
}

/// Whether a build offers the developer options (`host.debug`: the Web Inspector switch): a nightly and a dev build,
/// never a release.
pub fn debug_offered(channel: &str) -> bool {
    channel != "release"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_release_has_no_developer_options() {
        assert!(debug_offered("nightly") && debug_offered("dev"));
        assert!(!debug_offered("release"));
    }

    #[test]
    fn the_build_says_its_channel_and_only_what_it_has() {
        let release = BuildInfo::new("release", "0.4.0", "", "");
        assert_eq!(serde_json::to_value(&release).unwrap(), json!({"channel": "release", "version": "0.4.0"}));
        let nightly = BuildInfo::new("nightly", "0.4.1-nightly.3", "a1b2c3d", "2026-10-13");
        assert_eq!(
            serde_json::to_value(&nightly).unwrap(),
            json!({"channel": "nightly", "version": "0.4.1-nightly.3", "commit": "a1b2c3d", "date": "2026-10-13"})
        );
    }
}
