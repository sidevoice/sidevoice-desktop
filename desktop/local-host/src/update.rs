//! Keeping the installed service current within the minor the app pins (docs/LOCAL_HOST.md → Updates).
//!
//! The app works with one service minor, `X.Y`, which the bundled page declares (`service.pin.json`: `{"minor":
//! "0.20"}`; sidevoice-web owns the file, the app reads it from the bundle). The service is updated, in the background,
//! to the newest patch of that minor — never to another minor: a new minor can change what the page speaks, and arrives
//! with an app release that moves the pin. A service of a newer minor than the pin is never touched.
//!
//! This module is the pure part: what `localHost.version()` says ([`state`]), when the registry is asked again and when
//! a failed update is tried again ([`Updates`]), how far along an update is ([`progress`]). [`crate::host`] gathers the
//! facts and runs the update through the install ([`crate::install`]).

use crate::install::{Progress, Step};
use crate::npm::Minor;
use semver::Version;
use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, Instant};

/// How often the registry is asked for the newest patch (and once at launch).
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How soon it is asked again when it did not answer.
pub const CHECK_RETRY: Duration = Duration::from_secs(15 * 60);
/// How long an automatic update whose installer ran and failed is not tried again for the same release: each try
/// restarts the service (and rolls back), so a release that cannot start is not retried every few minutes. A newer
/// release is tried at once.
pub const RETRY_AFTER: Duration = Duration::from_secs(6 * 3600);
/// How long one that failed before its installer ran — the registry or the download, or a cancel — waits: nothing was
/// changed on this computer, so it is tried again soon.
pub const RETRY_SOON: Duration = Duration::from_secs(15 * 60);
/// How long one that the last check before its installer held (a call, a stop, a removal) waits: it changed nothing, and
/// must run soon after the call ends, but a cached poll that disagrees with that fresh check must not download the
/// release again at every pass.
pub const RETRY_GATE: Duration = Duration::from_secs(60);

/// The minor the bundled page pins: its `service.pin.json`, `{"minor": "X.Y"}`. Anything else — a file that is not
/// JSON (the bundle answers a missing file with its index page), no `minor`, a minor that is not `X.Y` — is no pin.
pub fn pin(bytes: &[u8]) -> Option<Minor> {
    let document: Value = serde_json::from_slice(bytes).ok()?;
    Minor::parse(document.get("minor")?.as_str()?)
}

/// What `localHost.version()` says of the service against the pin (`update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Update {
    /// Nothing behind it: an installed service of the pinned minor at the newest patch known.
    Current,
    /// Older than the newest patch of the pinned minor, or of an older minor: the update runs by itself.
    Available,
    /// An install or an update is running.
    Updating,
    /// Behind, and a call is on: the update runs once it ends.
    WaitingCall,
    /// Of a newer minor than the pin: never touched.
    NewerInstalled,
    /// The core speaks an API this app does not, and no update would change that (the service is of the pinned minor).
    Incompatible,
    /// No pin, or no installed version: nothing to compare.
    Unknown,
}

/// What [`state`] decides from.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    /// The minor the bundled page pins; `None` when it declares none (a web release before the pin).
    pub pinned: Option<Minor>,
    /// The installed service's version (`product_version`).
    pub installed: Option<&'a str>,
    /// The newest patch of the pinned minor, when the registry has said.
    pub latest: Option<&'a str>,
    /// The core's API is outside what this app speaks.
    pub incompatible: bool,
    /// Calls on now; `None` when that is not known (a core answers and does not say): nothing is updated then, as with a
    /// call on.
    pub calls: Option<u64>,
    /// An install or an update is running.
    pub working: bool,
}

/// Whether the installed service is older than what the app is to run: of an older minor than the pin, or of the pinned
/// one at an older patch than `latest` (when the registry has said). Not for a version that is not one.
pub fn is_behind(installed: &str, pinned: Minor, latest: Option<&str>) -> bool {
    let Ok(installed) = Version::parse(installed) else { return false };
    let mine = Minor { major: installed.major, minor: installed.minor };
    if mine != pinned {
        return mine < pinned;
    }
    latest.and_then(|latest| Version::parse(latest).ok()).is_some_and(|latest| installed < latest)
}

/// Whether the installed service is of a newer minor than the pin: never touched, never installed over.
pub fn is_newer(installed: &str, pinned: Minor) -> bool {
    Minor::of(installed).is_some_and(|mine| mine > pinned)
}

/// `update` as `localHost.version()` says it.
pub fn state(facts: Facts<'_>) -> Update {
    if facts.working {
        return Update::Updating;
    }
    let (Some(pinned), Some(installed)) = (facts.pinned, facts.installed.filter(|v| Version::parse(v).is_ok())) else {
        return Update::Unknown;
    };
    if is_newer(installed, pinned) {
        return Update::NewerInstalled;
    }
    match (is_behind(installed, pinned, facts.latest), facts.calls) {
        (true, Some(0)) => Update::Available,
        (true, _) => Update::WaitingCall,
        (false, _) if facts.incompatible => Update::Incompatible,
        (false, _) => Update::Current,
    }
}

/// How far along a running install or update is, 0 to 1: the download by its bytes up to 0.6 (nothing while the size is
/// not known), then each step's own mark. Monotonic through the steps, and two decimals.
pub fn progress(progress: &Progress) -> f64 {
    let fraction = match progress.step {
        Step::Download => match (progress.done, progress.total) {
            (Some(done), Some(total)) if total > 0 => 0.6 * (done as f64 / total as f64).min(1.0),
            _ => 0.0,
        },
        Step::Verification => 0.65,
        Step::Staging => 0.7,
        Step::ServiceStart => 0.85,
        Step::Pairing => 0.95,
    };
    (fraction * 100.0).round() / 100.0
}

/// What the app knows of the registry's newest patch and of its own automatic updates, between passes.
#[derive(Debug, Default)]
pub struct Updates {
    /// The newest patch of the pinned minor at the last answer of the registry; kept when a later ask fails.
    pub latest: Option<String>,
    /// When the registry was last asked, and whether it answered.
    checked: Option<(Instant, bool)>,
    /// The last update that failed: which release was the newest then, when, and how long it is left alone.
    failed: Option<(Option<String>, Instant, Duration)>,
}

impl Updates {
    /// Whether the registry is to be asked now: at launch, every [`CHECK_EVERY`], and [`CHECK_RETRY`] after a miss.
    pub fn check_due(&self, now: Instant) -> bool {
        match self.checked {
            None => true,
            Some((at, answered)) => {
                now.saturating_duration_since(at) >= if answered { CHECK_EVERY } else { CHECK_RETRY }
            }
        }
    }

    /// The registry's answer at `now`: the newest patch (or none in the minor), or `None` when it did not answer.
    pub fn checked(&mut self, now: Instant, answer: Option<Option<String>>) {
        self.checked = Some((now, answer.is_some()));
        if let Some(latest) = answer {
            self.latest = latest;
        }
    }

    /// An update to the newest patch known failed at `now`, and is left alone for `hold` ([`RETRY_AFTER`] when its
    /// installer ran, [`RETRY_SOON`] when it did not).
    pub fn update_failed(&mut self, now: Instant, hold: Duration) {
        self.failed = Some((self.latest.clone(), now, hold));
    }

    /// Whether the failed update is to be left alone for now: the same release is the newest, and its hold has not
    /// passed.
    pub fn holding_off(&self, now: Instant) -> bool {
        self.failed
            .as_ref()
            .is_some_and(|(latest, at, hold)| *latest == self.latest && now.saturating_duration_since(*at) < *hold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: Minor = Minor { major: 0, minor: 20 };

    fn facts<'a>(installed: Option<&'a str>, latest: Option<&'a str>) -> Facts<'a> {
        Facts { pinned: Some(PINNED), installed, latest, incompatible: false, calls: Some(0), working: false }
    }

    #[test]
    fn the_pin_is_the_minor_the_file_declares() {
        assert_eq!(pin(br#"{ "minor": "0.20" }"#), Some(PINNED));
        assert_eq!(pin(br#"{"minor":"0.20","note":"x"}"#), Some(PINNED));
        for bad in [
            &b""[..],
            b"<!doctype html><html></html>",
            b"{}",
            b"[]",
            br#"{"minor":20}"#,
            br#"{"minor":"0.20.1"}"#,
            br#"{"minor":"v0.20"}"#,
            br#"{"minor":""}"#,
            br#"{"minor":null}"#,
        ] {
            assert_eq!(pin(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn an_older_patch_of_the_pinned_minor_is_available() {
        assert_eq!(state(facts(Some("0.20.0"), Some("0.20.3"))), Update::Available);
        assert_eq!(state(facts(Some("0.20.2"), Some("0.20.3"))), Update::Available);
        assert_eq!(state(facts(Some("0.20.3"), Some("0.20.3"))), Update::Current);
        assert_eq!(state(facts(Some("0.20.9"), Some("0.20.3"))), Update::Current, "ahead of the registry: left alone");
    }

    #[test]
    fn patches_are_compared_as_numbers() {
        assert_eq!(state(facts(Some("0.20.9"), Some("0.20.10"))), Update::Available);
        assert_eq!(state(facts(Some("0.20.10"), Some("0.20.9"))), Update::Current);
    }

    #[test]
    fn an_older_minor_is_available_even_when_the_registry_has_not_said() {
        assert_eq!(state(facts(Some("0.19.7"), None)), Update::Available);
        assert_eq!(state(facts(Some("0.9.0"), Some("0.20.1"))), Update::Available, "9 is before 20");
    }

    #[test]
    fn a_service_of_the_pinned_minor_is_current_until_the_registry_says_otherwise() {
        assert_eq!(state(facts(Some("0.20.0"), None)), Update::Current);
    }

    #[test]
    fn a_newer_minor_is_never_touched() {
        for installed in ["0.21.0", "0.21.7", "1.0.0", "0.100.0"] {
            assert_eq!(state(facts(Some(installed), Some("0.20.3"))), Update::NewerInstalled, "{installed}");
            assert_eq!(state(facts(Some(installed), None)), Update::NewerInstalled, "{installed}");
        }
        let mut busy = facts(Some("0.21.0"), Some("0.20.3"));
        busy.calls = Some(2);
        assert_eq!(state(busy), Update::NewerInstalled, "not even waiting for a call");
    }

    #[test]
    fn behind_with_a_call_on_waits_for_it() {
        let mut behind = facts(Some("0.20.0"), Some("0.20.3"));
        behind.calls = Some(1);
        assert_eq!(state(behind), Update::WaitingCall);
        let mut older_minor = facts(Some("0.19.0"), None);
        older_minor.calls = Some(3);
        assert_eq!(state(older_minor), Update::WaitingCall);
        let mut current = facts(Some("0.20.3"), Some("0.20.3"));
        current.calls = Some(3);
        assert_eq!(state(current), Update::Current, "a call is no reason to say anything when there is nothing to do");
    }

    #[test]
    fn an_unknown_count_of_calls_is_never_taken_for_none() {
        let mut behind = facts(Some("0.20.0"), Some("0.20.3"));
        behind.calls = None;
        assert_eq!(state(behind), Update::WaitingCall, "behind, and a call may be on: it waits");
        let mut older_minor = facts(Some("0.19.0"), None);
        older_minor.calls = None;
        assert_eq!(state(older_minor), Update::WaitingCall);
        let mut current = facts(Some("0.20.3"), Some("0.20.3"));
        current.calls = None;
        assert_eq!(state(current), Update::Current, "nothing to do is nothing to wait for");
        let mut newer = facts(Some("0.21.0"), None);
        newer.calls = None;
        assert_eq!(state(newer), Update::NewerInstalled);
    }

    #[test]
    fn newer_and_behind_are_read_from_the_minor_and_the_patch() {
        assert!(is_newer("0.21.0", PINNED) && is_newer("1.0.0", PINNED) && is_newer("0.100.1", PINNED));
        assert!(!is_newer("0.20.9", PINNED) && !is_newer("0.19.9", PINNED) && !is_newer("nope", PINNED));
        assert!(is_behind("0.20.0", PINNED, Some("0.20.1")) && is_behind("0.19.0", PINNED, None));
        assert!(!is_behind("0.20.1", PINNED, Some("0.20.1")) && !is_behind("0.20.0", PINNED, None));
        assert!(!is_behind("0.21.0", PINNED, Some("0.20.1")) && !is_behind("nope", PINNED, Some("0.20.1")));
    }

    #[test]
    fn a_running_install_is_updating_whatever_else_is_known() {
        let mut working = facts(Some("0.20.0"), Some("0.20.3"));
        working.working = true;
        assert_eq!(state(working), Update::Updating);
        working.installed = None;
        assert_eq!(state(working), Update::Updating);
        working.pinned = None;
        assert_eq!(state(working), Update::Updating);
    }

    #[test]
    fn without_a_pin_or_an_installed_version_nothing_is_compared() {
        let mut unpinned = facts(Some("0.20.0"), Some("0.20.3"));
        unpinned.pinned = None;
        assert_eq!(state(unpinned), Update::Unknown);
        assert_eq!(state(facts(None, Some("0.20.3"))), Update::Unknown);
        assert_eq!(state(facts(Some("not a version"), Some("0.20.3"))), Update::Unknown);
    }

    #[test]
    fn an_incompatible_core_of_the_pinned_minor_is_said_and_an_older_one_is_updated() {
        let mut current = facts(Some("0.20.3"), Some("0.20.3"));
        current.incompatible = true;
        assert_eq!(state(current), Update::Incompatible);
        let mut older = facts(Some("0.19.0"), Some("0.20.3"));
        older.incompatible = true;
        assert_eq!(state(older), Update::Available, "the update is what would fix it");
    }

    #[test]
    fn a_pre_release_is_behind_its_own_release() {
        assert_eq!(state(facts(Some("0.20.3-rc.1"), Some("0.20.3"))), Update::Available);
        assert_eq!(state(facts(Some("0.20.3+abc123"), Some("0.20.3"))), Update::Current, "a build is the same release");
    }

    #[test]
    fn the_progress_runs_forward_through_the_steps() {
        let at = |step, done, total| progress(&Progress { step, done, total, cancellable: true });
        assert_eq!(at(Step::Download, None, None), 0.0);
        assert_eq!(at(Step::Download, Some(0), Some(100)), 0.0);
        assert_eq!(at(Step::Download, Some(50), Some(100)), 0.3);
        assert_eq!(at(Step::Download, Some(500), Some(100)), 0.6, "never past the download's share");
        assert_eq!(at(Step::Download, Some(5), Some(0)), 0.0, "an empty size is no size");
        let steps = [
            at(Step::Download, Some(100), Some(100)),
            at(Step::Verification, None, None),
            at(Step::Staging, None, None),
            at(Step::ServiceStart, None, None),
            at(Step::Pairing, None, None),
        ];
        assert!(steps.windows(2).all(|pair| pair[0] < pair[1]), "{steps:?}");
        assert!(steps.iter().all(|fraction| (0.0..=1.0).contains(fraction)));
    }

    #[test]
    fn the_registry_is_asked_at_launch_every_six_hours_and_sooner_after_a_miss() {
        let start = Instant::now();
        let mut updates = Updates::default();
        assert!(updates.check_due(start), "at launch");
        updates.checked(start, Some(Some("0.20.3".into())));
        assert_eq!(updates.latest.as_deref(), Some("0.20.3"));
        assert!(!updates.check_due(start + CHECK_EVERY - Duration::from_secs(1)));
        assert!(updates.check_due(start + CHECK_EVERY));
        updates.checked(start + CHECK_EVERY, None);
        assert_eq!(updates.latest.as_deref(), Some("0.20.3"), "a miss keeps what was known");
        assert!(!updates.check_due(start + CHECK_EVERY + CHECK_RETRY - Duration::from_secs(1)));
        assert!(updates.check_due(start + CHECK_EVERY + CHECK_RETRY));
        updates.checked(start + CHECK_EVERY + CHECK_RETRY, Some(None));
        assert_eq!(updates.latest, None, "the registry saying there is none in the minor is an answer");
    }

    #[test]
    fn a_failed_update_waits_unless_a_newer_release_is_out() {
        let start = Instant::now();
        let mut updates = Updates::default();
        updates.checked(start, Some(Some("0.20.3".into())));
        assert!(!updates.holding_off(start), "nothing failed");
        updates.update_failed(start, RETRY_AFTER);
        assert!(updates.holding_off(start + Duration::from_secs(60)));
        assert!(updates.holding_off(start + RETRY_AFTER - Duration::from_secs(1)));
        assert!(!updates.holding_off(start + RETRY_AFTER), "tried again after the wait");
        updates.checked(start + Duration::from_secs(60), Some(Some("0.20.4".into())));
        assert!(!updates.holding_off(start + Duration::from_secs(61)), "a newer release is another try");
    }

    #[test]
    fn a_failure_before_the_installer_ran_is_tried_again_in_fifteen_minutes() {
        let start = Instant::now();
        let mut updates = Updates::default();
        updates.checked(start, Some(Some("0.20.3".into())));
        updates.update_failed(start, RETRY_SOON);
        assert_eq!(RETRY_SOON, Duration::from_secs(900));
        assert_eq!(RETRY_GATE, Duration::from_secs(60));
        assert!(RETRY_GATE < RETRY_SOON && RETRY_SOON < RETRY_AFTER);
        assert!(updates.holding_off(start + RETRY_SOON - Duration::from_secs(1)));
        assert!(!updates.holding_off(start + RETRY_SOON));
        updates.update_failed(start, RETRY_AFTER);
        assert!(updates.holding_off(start + RETRY_SOON), "an installer's failure is held for much longer");
    }
}
