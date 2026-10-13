//! What this build of the app is (`window.__sidevoiceDesktop.host.build`, docs/BRIDGE.md → "The build"): a numbered
//! release, a nightly of `main`, or a development build, decided when it was built (the app's build.rs reads
//! `SIDEVOICE_BUILD_CHANNEL`, which `cargo xtask` sets; RELEASING.md → "Channels"). Only a release has the updater.

use serde::Serialize;

/// Which builds the app comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// A numbered release (`vX.Y.Z`), built by the release's run: the only one that updates itself.
    Release,
    /// The rolling `nightly` pre-release, built from `main`.
    Nightly,
    /// Anything else: a laptop, a pull request, a manual run (its `sign` dry runs included).
    #[default]
    Dev,
}

impl Channel {
    /// The channel the build script was given (`release`, `nightly`, `dev`); `None` for anything else.
    pub fn parse(name: &str) -> Option<Channel> {
        match name {
            "release" => Some(Channel::Release),
            "nightly" => Some(Channel::Nightly),
            "dev" => Some(Channel::Dev),
            _ => None,
        }
    }
}

/// `host.build`: `{channel, version, commit?, date?}`. A nightly carries the last release's version (the one `main`
/// is at) with its short commit and the day it was built (`YYYY-MM-DD`, UTC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    pub channel: Channel,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

impl BuildInfo {
    /// From what the build script passed on (an empty commit or date is none). An unknown channel is a development
    /// build: the build script refuses one, so this is only ever a fallback.
    pub fn new(channel: &str, version: &str, commit: &str, date: &str) -> BuildInfo {
        let given = |s: &str| (!s.is_empty()).then(|| s.to_owned());
        BuildInfo {
            channel: Channel::parse(channel).unwrap_or_default(),
            version: version.to_owned(),
            commit: given(commit),
            date: given(date),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_says_its_channel_and_only_what_it_has() {
        let release = BuildInfo::new("release", "0.4.0", "", "");
        assert_eq!(
            serde_json::to_value(&release).unwrap(),
            serde_json::json!({"channel": "release", "version": "0.4.0"})
        );
        let nightly = BuildInfo::new("nightly", "0.4.0", "a1b2c3d", "2026-10-13");
        assert_eq!(
            serde_json::to_value(&nightly).unwrap(),
            serde_json::json!({"channel": "nightly", "version": "0.4.0", "commit": "a1b2c3d", "date": "2026-10-13"})
        );
        assert_eq!(BuildInfo::new("dev", "0.4.0", "", "").channel, Channel::Dev);
        assert_eq!(BuildInfo::new("beta", "0.4.0", "", "").channel, Channel::Dev, "never a release by accident");
        assert_eq!(Channel::parse("Release"), None);
    }
}
