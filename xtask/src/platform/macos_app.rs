//! What the macOS app's steps share: where the built app is, its data, how it is started, and what a build must not
//! read from the environment.

use std::path::{Path, PathBuf};

use crate::util::{ensure, repo, scratch_home, Cmd};
use crate::Result;

pub fn release_target() -> &'static sidevoice_platform::target::ReleaseTarget {
    sidevoice_platform::target::for_platform("macos", "aarch64").expect("published macOS target")
}
/// The app's identifier (tauri.conf.json): its directory under Application Support.
pub const IDENTIFIER: &str = "dev.sidevoice.desktop";
/// Where the stand-in node (test/fixtures/fake-node.py) listens.
pub const FAKE_NODE_PORT: &str = "8768";

/// The bundle every `release` command works on, as `tauri build --target aarch64-apple-darwin` leaves it.
pub fn app() -> PathBuf {
    repo().join("src-tauri/target").join(release_target().triple).join("release/bundle/macos/Sidevoice.app")
}

/// The app's binary in [`app`].
pub fn binary() -> PathBuf {
    app().join("Contents/MacOS/sidevoice-desktop")
}

/// The app's settings and data under the person's home (`~/Library/Application Support/dev.sidevoice.desktop`).
pub fn app_data(home: &Path) -> PathBuf {
    home.join("Library/Application Support").join(IDENTIFIER)
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| "HOME is not set".into())
}

/// The app's binary, its diagnostics on (`SIDEVOICE_DEBUG=1`), with `env`.
pub fn app_command(env: &[(&str, &str)]) -> Cmd {
    let mut cmd = Cmd::new(binary()).env("SIDEVOICE_DEBUG", "1");
    for (key, value) in env {
        cmd = cmd.env(key, value);
    }
    cmd
}

/// The stand-in node (test/fixtures/fake-node.py) on loopback, logging every request with its Origin.
pub fn fake_node(log: &Path) -> Result<crate::util::Background> {
    Cmd::new("python3").args(["-u", "test/fixtures/fake-node.py", FAKE_NODE_PORT]).spawn(log)
}

/// The home a step starts the app with (`util::scratch_home` decides which): the person's on a runner or a scratch one.
/// (The app's provider keys are in the login keychain, which a home does not change: `probe probe` expects none there,
/// as on a runner.)
pub struct AppHome {
    pub home: PathBuf,
    scratch: bool,
}

impl AppHome {
    pub fn new(step: &str) -> Result<AppHome> {
        match scratch_home(step, Path::new("/tmp"))? {
            Some(home) => Ok(AppHome { home, scratch: true }),
            None => Ok(AppHome { home: home()?, scratch: false }),
        }
    }

    /// The app's binary with this home, its diagnostics on, with `env`.
    pub fn app(&self, env: &[(&str, &str)]) -> Cmd {
        let cmd = app_command(env);
        if self.scratch {
            cmd.env("HOME", &self.home).env_remove("SIDEVOICE_DATA_DIR")
        } else {
            cmd
        }
    }
}

/// The signing and notarization variables a build would otherwise read: an unsigned or probe build never signs with
/// the Developer ID, whatever this machine's environment holds (tauri.conf.json's ad-hoc identity then applies).
pub const SIGNING_ENV: &[&str] = &[
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "APPLE_SIGNING_IDENTITY",
    "APPLE_ID",
    "APPLE_PASSWORD",
    "APPLE_TEAM_ID",
    "APPLE_API_KEY",
    "APPLE_API_ISSUER",
    "APPLE_API_KEY_PATH",
    "APPLE_API_KEY_P8",
];

/// `cmd` without [`SIGNING_ENV`].
pub fn unsigned(cmd: Cmd) -> Cmd {
    SIGNING_ENV.iter().fold(cmd, |cmd, name| cmd.env_remove(name))
}

/// What only the probe build carries (src-tauri/src/probe.rs): never in a release app.
pub const PROBE_ONLY: &[&str] = &[
    "SIDEVOICE_DEBUG_PAGE",
    "probe-engine",
    "probe-card",
    "SIDEVOICE_DEBUG_LOCAL_HOST_FLOW",
    "local-host-flow",
    "SIDEVOICE_DEBUG_EXIT",
];
/// The paths in `security list-keychains`' output: one per line, indented and quoted.
pub fn keychain_list(text: &str) -> Vec<String> {
    text.lines().map(|line| line.trim().trim_matches('"').to_owned()).filter(|path| !path.is_empty()).collect()
}

/// What `codesign -dvvv` says of a signature, as far as the checks go.
#[derive(Debug, Default, PartialEq)]
pub struct Signature {
    pub adhoc: bool,
    pub runtime: bool,
    pub developer_id: bool,
    pub team: Option<String>,
    pub timestamp: bool,
}

impl Signature {
    pub fn parse(text: &str) -> Signature {
        let mut signature = Signature {
            adhoc: text.contains("Signature=adhoc"),
            runtime: text.contains("runtime"),
            ..Signature::default()
        };
        for line in text.lines() {
            signature.developer_id |= line.starts_with("Authority=Developer ID Application: ");
            signature.timestamp |= line.starts_with("Timestamp=");
            if let Some(team) = line.strip_prefix("TeamIdentifier=") {
                signature.team.get_or_insert_with(|| team.to_owned());
            }
        }
        signature
    }

    /// The checks on the app's signature: the hardened runtime always; then ad-hoc, or (with the team) the
    /// Developer ID's, this team's, with a secure timestamp.
    pub fn check(text: &str, team: Option<&str>) -> Result<()> {
        let signature = Signature::parse(text);
        ensure(signature.runtime, "not signed with the hardened runtime")?;
        let Some(team) = team else {
            return ensure(signature.adhoc, "not ad-hoc signed");
        };
        ensure(signature.developer_id, "not signed by a Developer ID Application")?;
        ensure(
            text.lines().any(|line| line == format!("TeamIdentifier={team}")),
            format!(
                "not signed by the team APPLE_TEAM_ID names (TeamIdentifier={})",
                signature.team.unwrap_or_default()
            ),
        )?;
        ensure(signature.timestamp, "no secure timestamp")?;
        ensure(!signature.adhoc, "ad-hoc signed in a signed build")
    }
}

/// Whether `spctl -a -vv` accepted it as a notarized Developer ID app or image.
pub fn notarized(spctl: &str) -> bool {
    spctl.contains("source=Notarized Developer ID")
}

/// `notarytool submit --output-format json`'s status.
pub fn notary_status(json: &str) -> Result<String> {
    let answer: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("notarytool's answer: {e}"))?;
    answer["status"].as_str().map(str::to_owned).ok_or_else(|| "notarytool's answer has no status".into())
}

/// Whether a path on the search list is a signing step's own keychain ([`SIGNING`]/[`KEYCHAIN`]), however the temporary
/// directory resolves (`/var` is `/private/var` on a Mac).
pub fn signing_keychain(path: &str) -> bool {
    Path::new(path).ends_with(Path::new(SIGNING).join(KEYCHAIN))
}

/// Where the signing steps keep their secrets ([`SecretDir`]): the keychain and the certificate, the notary's key.
pub const SIGNING: &str = "sidevoice-signing";
pub const KEYCHAIN: &str = "sidevoice-signing.keychain-db";

/// The screen region (`x,y,w,h`, the title bar's 28 points included) of a Finder window's bounds (`x1,y1,x2,y2`).
pub fn region(bounds: &str) -> Option<String> {
    let numbers: Vec<i64> = bounds.split(',').map(|n| n.trim().parse().ok()).collect::<Option<_>>()?;
    let [x1, y1, x2, y2] = numbers[..] else { return None };
    Some(format!("{x1},{},{},{}", y1 - 28, x2 - x1, y2 - y1 + 28))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::macos_app::SIGNING_ENV;

    const ADHOC: &str = "Executable=/x/Sidevoice.app/Contents/MacOS/sidevoice-desktop
Identifier=dev.sidevoice.desktop
Format=app bundle with Mach-O thin (arm64)
CodeDirectory v=20500 size=1 flags=0x10002(adhoc,runtime) hashes=1+7 location=embedded
Signature=adhoc
TeamIdentifier=not set
";

    const SIGNED: &str = "Executable=/x/Sidevoice.app/Contents/MacOS/sidevoice-desktop
CodeDirectory v=20500 size=1 flags=0x10000(runtime) hashes=1+7 location=embedded
Signature size=9045
Authority=Developer ID Application: Example Person (AB12CD34EF)
Authority=Developer ID Certification Authority
Authority=Apple Root CA
Timestamp=12 Oct 2026 at 10:00:00
TeamIdentifier=AB12CD34EF
";

    #[test]
    fn an_adhoc_signature_passes_only_as_adhoc() {
        assert_eq!(Signature::check(ADHOC, None), Ok(()));
        assert!(Signature::check(ADHOC, Some("AB12CD34EF")).is_err());
        assert!(Signature::check(&ADHOC.replace("adhoc,runtime", "adhoc"), None).is_err());
    }

    #[test]
    fn a_developer_id_signature_needs_the_team_and_a_timestamp() {
        assert_eq!(Signature::check(SIGNED, Some("AB12CD34EF")), Ok(()));
        assert!(Signature::check(SIGNED, None).is_err(), "a signed app is not ad-hoc");
        let other_team = Signature::check(SIGNED, Some("ZZ99")).unwrap_err();
        assert!(other_team.contains("AB12CD34EF"), "{other_team}");
        assert!(Signature::check(SIGNED, Some("AB12CD34")).is_err(), "the whole line, not a prefix");
        assert!(Signature::check(&SIGNED.replace("Timestamp=", "Signed Time="), Some("AB12CD34EF")).is_err());
        assert!(Signature::check(&SIGNED.replace("Developer ID Application", "Apple Development"), Some("AB12CD34EF"))
            .is_err());
        let both = format!("{SIGNED}Signature=adhoc\n");
        assert_eq!(Signature::check(&both, Some("AB12CD34EF")).unwrap_err(), "ad-hoc signed in a signed build");
    }

    #[test]
    fn gatekeeper_and_the_notary_are_read() {
        assert!(notarized(
            "/x/Sidevoice.app: accepted\nsource=Notarized Developer ID\norigin=Developer ID Application: X"
        ));
        assert!(!notarized("/x/Sidevoice.app: rejected\nsource=Unnotarized Developer ID"));
        assert_eq!(notary_status(r#"{"id":"1","status":"Accepted","message":"ok"}"#).unwrap(), "Accepted");
        assert_eq!(notary_status(r#"{"status":"Invalid"}"#).unwrap(), "Invalid");
        assert!(notary_status("Error: HTTP 401").is_err());
    }

    #[test]
    fn a_finder_window_becomes_a_screen_region() {
        assert_eq!(region("100,128,740,528").as_deref(), Some("100,100,640,428"));
        assert_eq!(region("0,28,1440,900").as_deref(), Some("0,0,1440,900"));
        assert_eq!(region("missing value"), None);
        assert_eq!(region("1,2,3"), None);
    }

    #[test]
    fn the_keychain_list_is_read_unquoted() {
        let out =
            "    \"/Users/me/Library/Keychains/login.keychain-db\"\n    \"/private/var/folders/x/T/abc.keychain-db\"\n";
        assert_eq!(
            keychain_list(out),
            ["/Users/me/Library/Keychains/login.keychain-db", "/private/var/folders/x/T/abc.keychain-db"]
        );
    }

    #[test]
    fn the_signing_keychain_is_told_from_the_others_on_the_list() {
        assert!(signing_keychain("/Users/runner/work/_temp/sidevoice-signing/sidevoice-signing.keychain-db"));
        assert!(signing_keychain("/private/var/folders/x/T/sidevoice-signing/sidevoice-signing.keychain-db"));
        assert!(!signing_keychain("/Users/me/Library/Keychains/login.keychain-db"));
        assert!(!signing_keychain("/Users/runner/Library/Keychains/Nt0IPhmZAIYKjzRR.keychain-db"));
    }

    #[test]
    fn an_unsigned_build_drops_every_signing_variable() {
        assert!(SIGNING_ENV.contains(&"APPLE_SIGNING_IDENTITY") && SIGNING_ENV.contains(&"APPLE_API_KEY_P8"));
    }
}
