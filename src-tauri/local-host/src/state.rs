//! What the bridge reports about the local host (`localHost.state()`, SEAMS §5), from what the app observed: the
//! node service's state (`node.status`, else `service status --json`), the core on the socket, and the app's own
//! pairing with it. Pure: the poll gathers, this decides.
//!
//! | Observed | State |
//! |---|---|
//! | the core's directory or peer fails a check | `failed` with `identity.unsafe-directory` / `peer.uid-mismatch` |
//! | a core answers with an `api` outside [`API`] (or the service reports one) | `incompatible` |
//! | a core answers; the app's token is refused (same fingerprint) | `refused` (until `reconnect()`) |
//! | a core answers; pairing or storing the token failed | `failed` with that cause (`app.storage`, `identity.mismatch`) |
//! | a core answers; the app is paired and the core accepts the token | `running` |
//! | a core answers; the app has not paired yet | `starting` |
//! | no core: the service's state | `absent`, `not-installed`, `stopped-by-person`, `starting`, `backoff`, `failed`, `service-failed` as reported; `running` or `stopped` → `starting` (a core is on its way or going; the next poll tells) |
//! | no core, no connector, no install | `absent` |
//!
//! `installing` is R4's.

use crate::core_socket::{CoreError, Health};
use crate::Refusal;
use serde::Serialize;
use serde_json::{json, Value};
use std::ops::RangeInclusive;

/// The client API versions this app speaks (sidevoice design §4.3): the core's `api` must be in it.
pub const API: RangeInclusive<i64> = 1..=1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Absent,
    NotInstalled,
    StoppedByPerson,
    Starting,
    Backoff,
    Running,
    Failed,
    ServiceFailed,
    Refused,
    Incompatible,
}

/// `{state, failure?, core?, service?, calls?}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calls: Option<u64>,
}

impl Report {
    pub fn new(state: State) -> Self {
        Report { state, failure: None, core: None, service: None, calls: None }
    }
}

/// Where the app's own pairing with the answering core stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Not paired with this core yet (or being paired now).
    Unpaired,
    /// Paired, its identity proven this launch and the token accepted.
    Paired,
    /// The core no longer accepts the token, with the same fingerprint: only `reconnect()` pairs again.
    Refused,
    /// Pairing or keeping it failed: `app.storage`, `identity.mismatch`, … Only `reconnect()` or a new core launch
    /// tries again.
    Failed(Refusal),
}

/// What one poll saw.
#[derive(Debug, Clone, Copy)]
pub struct Observed<'a> {
    /// `node.status`'s result, or `service status --json`'s answer; `None` with neither.
    pub status: Option<&'a Value>,
    pub core: Result<&'a Health, &'a CoreError>,
    pub link: &'a Link,
}

fn compatible(api: Option<i64>) -> bool {
    api.is_some_and(|api| API.contains(&api))
}

fn health_core(health: &Health) -> Value {
    json!({ "pid": health.pid, "version": health.version, "api": health.api, "launch_id": health.launch_id })
}

pub fn report(observed: Observed<'_>) -> Report {
    let status = observed.status;
    let field = |name: &str| status.and_then(|s| s.get(name)).filter(|v| !v.is_null());
    let mut report = Report::new(State::Absent);
    report.service = field("service").and_then(Value::as_str).map(str::to_string);
    report.calls = field("calls").and_then(Value::as_u64);
    report.core = field("core").cloned();

    match observed.core {
        Err(CoreError::Unsafe(refusal)) => {
            report.state = State::Failed;
            report.failure = Some(json!(refusal));
        }
        Ok(health) => {
            report.core = Some(health_core(health));
            report.calls = health.calls.or(report.calls);
            report.state = match observed.link {
                _ if !compatible(health.api) => State::Incompatible,
                Link::Refused => State::Refused,
                Link::Failed(refusal) => {
                    report.failure = Some(json!(refusal));
                    State::Failed
                }
                Link::Paired => State::Running,
                Link::Unpaired => State::Starting,
            };
        }
        Err(answer) => {
            let service_api = report.core.as_ref().and_then(|c| c.get("api")).and_then(Value::as_i64);
            report.failure = field("failure").cloned();
            report.state = match field("state").and_then(Value::as_str) {
                _ if service_api.is_some() && !compatible(service_api) => State::Incompatible,
                Some("absent") => State::Absent,
                Some("not-installed") => State::NotInstalled,
                Some("stopped-by-person") => State::StoppedByPerson,
                Some("starting" | "running" | "stopped") => State::Starting,
                Some("backoff") => State::Backoff,
                Some("failed") => State::Failed,
                Some("service-failed") => State::ServiceFailed,
                Some(other) => {
                    report.failure = Some(
                        json!({ "key": "status.unknown", "message": format!("The service reported state {other:?}.") }),
                    );
                    State::Failed
                }
                // Neither a connector nor the CLI could say; a socket that answered nonsense is a failure.
                None => match answer {
                    CoreError::Failed(refusal) => {
                        report.failure = Some(json!(refusal));
                        State::Failed
                    }
                    _ => State::Absent,
                },
            };
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(api: Option<i64>) -> Health {
        Health {
            launch_id: Some("L1".into()),
            pid: Some(42),
            version: Some("0.2.0".into()),
            api,
            fingerprint: "fp".into(),
            public_key: "key".into(),
            host: Some("mac".into()),
            calls: Some(1),
        }
    }

    fn state(status: Option<Value>, core: Result<&Health, &CoreError>, link: &Link) -> Report {
        report(Observed { status: status.as_ref(), core, link })
    }

    #[test]
    fn a_core_that_answers_decides_with_the_apps_pairing() {
        let h = health(Some(1));
        assert_eq!(state(None, Ok(&h), &Link::Paired).state, State::Running);
        assert_eq!(state(None, Ok(&h), &Link::Unpaired).state, State::Starting);
        assert_eq!(state(None, Ok(&h), &Link::Refused).state, State::Refused);
        let storage = Refusal::new("app.storage", "m");
        let failed = state(None, Ok(&h), &Link::Failed(storage));
        assert_eq!(failed.state, State::Failed);
        assert_eq!(failed.failure.unwrap()["key"], "app.storage");
        let running = state(Some(json!({"state": "running", "service": "launchd", "calls": 0})), Ok(&h), &Link::Paired);
        assert_eq!(running.service.as_deref(), Some("launchd"));
        assert_eq!(running.calls, Some(1), "the core's own count wins");
        assert_eq!(running.core.unwrap()["launch_id"], "L1");
    }

    #[test]
    fn an_api_outside_the_range_is_incompatible_whatever_else() {
        for api in [Some(0), Some(2), None] {
            let h = health(api);
            assert_eq!(state(None, Ok(&h), &Link::Paired).state, State::Incompatible, "{api:?}");
        }
        let status = json!({"state": "running", "core": {"api": 7}});
        assert_eq!(state(Some(status), Err(&CoreError::Absent), &Link::Unpaired).state, State::Incompatible);
    }

    #[test]
    fn without_a_core_the_service_says() {
        for (reported, expected) in [
            ("absent", State::Absent),
            ("not-installed", State::NotInstalled),
            ("stopped-by-person", State::StoppedByPerson),
            ("starting", State::Starting),
            ("running", State::Starting),
            ("stopped", State::Starting),
            ("backoff", State::Backoff),
            ("failed", State::Failed),
            ("service-failed", State::ServiceFailed),
        ] {
            let status = json!({"state": reported, "failure": {"key": "start-limit"}, "service": "launchd"});
            let got = state(Some(status), Err(&CoreError::Absent), &Link::Unpaired);
            assert_eq!(got.state, expected, "{reported}");
            assert_eq!(got.failure.unwrap()["key"], "start-limit");
        }
        let odd = state(Some(json!({"state": "dancing"})), Err(&CoreError::Absent), &Link::Unpaired);
        assert_eq!((odd.state, odd.failure.unwrap()["key"].clone()), (State::Failed, json!("status.unknown")));
    }

    #[test]
    fn nothing_at_all_is_absent_and_an_unsafe_socket_is_failed() {
        assert_eq!(state(None, Err(&CoreError::Absent), &Link::Unpaired), Report::new(State::Absent));
        let unsafe_dir = CoreError::Unsafe(Refusal::new("identity.unsafe-directory", "m"));
        let got = state(Some(json!({"state": "running"})), Err(&unsafe_dir), &Link::Paired);
        assert_eq!(
            (got.state, got.failure.unwrap()["key"].clone()),
            (State::Failed, json!("identity.unsafe-directory"))
        );
        let peer = CoreError::Unsafe(Refusal::new("peer.uid-mismatch", "m"));
        assert_eq!(state(None, Err(&peer), &Link::Unpaired).state, State::Failed);
        let garbled = CoreError::Failed(Refusal::new("core.unexpected", "m"));
        assert_eq!(state(None, Err(&garbled), &Link::Unpaired).state, State::Failed);
    }

    #[test]
    fn states_serialise_as_the_bridge_names_them() {
        let names: Vec<Value> = [
            State::Absent,
            State::NotInstalled,
            State::StoppedByPerson,
            State::Starting,
            State::Backoff,
            State::Running,
            State::Failed,
            State::ServiceFailed,
            State::Refused,
            State::Incompatible,
        ]
        .iter()
        .map(|s| json!(s))
        .collect();
        assert_eq!(
            names,
            [
                "absent",
                "not-installed",
                "stopped-by-person",
                "starting",
                "backoff",
                "running",
                "failed",
                "service-failed",
                "refused",
                "incompatible"
            ]
        );
        assert_eq!(json!(Report::new(State::Running)), json!({"state": "running"}));
    }
}
