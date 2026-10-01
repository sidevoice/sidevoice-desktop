//! The core's private listener, `C/local.sock` (SEAMS §2), as the app uses it: every connect checked first
//! ([`checks`]), one request per connection.
//!
//! - `GET /api/local/health` → who answers: launch, version, `api`, fingerprint and key, calls.
//! - `GET /api/device/identity?nonce=` → the core signs a fresh nonce; checked against the pinned key.
//! - `POST /api/device/local/pair {name}` → a device token for this app (the core revokes the previous local one).
//! - `DELETE /api/device/local` → this app's local device revoked.
//! - Any authenticated route with the token, to tell whether the core still accepts it.

use crate::checks::{self, Check};
use crate::http::{self, Response};
use crate::identity;
use crate::Refusal;
use serde::{Deserialize, Serialize};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// How long the app waits for the core's own answers. Health is the readiness probe: 2 s (design §4.2).
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// What `GET /api/local/health` says (SEAMS §2). Fields the app does not need are not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    #[serde(default)]
    pub launch_id: Option<String>,
    #[serde(default)]
    pub pid: Option<u64>,
    #[serde(default)]
    pub version: Option<String>,
    /// The client API's version: the app supports a range ([`crate::state::API`]).
    #[serde(default)]
    pub api: Option<i64>,
    pub fingerprint: String,
    pub public_key: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub calls: Option<u64>,
}

/// The node a pairing names (`node` of `POST /api/device/local/pair`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Node {
    pub fingerprint: String,
    pub public_key: String,
    #[serde(default)]
    pub host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Paired {
    pub device_id: String,
    pub token: String,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// Nobody listens on the socket, or the directory is not there.
    Absent,
    /// The directory or the peer failed a check: the app does not talk to it.
    Unsafe(Refusal),
    /// It answered, but not what the contract says.
    Failed(Refusal),
}

impl From<Check> for CoreError {
    fn from(check: Check) -> Self {
        match check {
            Check::Missing => CoreError::Absent,
            Check::Unsafe(refusal) => CoreError::Unsafe(refusal),
        }
    }
}

fn failed(why: impl Into<String>) -> CoreError {
    CoreError::Failed(Refusal::new("core.unexpected", why))
}

/// `C`, and its socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreSocket {
    pub dir: PathBuf,
    pub socket: PathBuf,
}

impl CoreSocket {
    pub fn new(dirs: &crate::paths::DataDirs) -> Self {
        CoreSocket { dir: dirs.core(), socket: dirs.core_socket() }
    }

    /// A connection to the core, after the directory and peer checks, with `timeout` on reads and writes.
    pub fn connect(&self, timeout: Option<Duration>) -> Result<UnixStream, CoreError> {
        let stream = checks::connect(&self.dir, &self.socket, checks::private_dir)?;
        stream.set_read_timeout(timeout).and_then(|_| stream.set_write_timeout(timeout)).map_err(|e| failed(e.to_string()))?;
        Ok(stream)
    }

    fn call(
        &self,
        method: &str,
        target: &str,
        fields: &[(&str, &str)],
        body: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Response, CoreError> {
        let mut stream = self.connect(Some(timeout))?;
        http::exchange(&mut stream, method, target, fields, body).map_err(|e| failed(format!("{method} {target}: {e}")))
    }

    pub fn health(&self) -> Result<Health, CoreError> {
        let answer = self.call("GET", "/api/local/health", &[], None, HEALTH_TIMEOUT)?;
        if answer.status != 200 {
            return Err(failed(format!("GET /api/local/health answered {}", answer.status)));
        }
        answer.json().map_err(|e| failed(format!("GET /api/local/health: {e}")))
    }

    /// Whether the core signs a fresh nonce with `public_key`'s private half.
    pub fn proves(&self, public_key: &str) -> Result<bool, CoreError> {
        let nonce = identity::nonce();
        let answer = self.call("GET", &format!("/api/device/identity?nonce={nonce}"), &[], None, CALL_TIMEOUT)?;
        if answer.status != 200 {
            return Err(failed(format!("GET /api/device/identity answered {}", answer.status)));
        }
        let proof: serde_json::Value = answer.json().map_err(|e| failed(e.to_string()))?;
        let signature = proof.get("signature").and_then(|s| s.as_str()).unwrap_or("");
        Ok(identity::verify(public_key, &nonce, signature))
    }

    pub fn pair(&self, name: &str) -> Result<Paired, CoreError> {
        let body = serde_json::to_vec(&serde_json::json!({ "name": name })).expect("serialises");
        let answer = self.call("POST", "/api/device/local/pair", &[], Some(&body), CALL_TIMEOUT)?;
        if answer.status != 200 {
            return Err(failed(format!("POST /api/device/local/pair answered {}", answer.status)));
        }
        answer.json().map_err(|e| failed(format!("POST /api/device/local/pair: {e}")))
    }

    /// Revokes this app's local device (the token in hand stops working, its calls end).
    pub fn unpair(&self) -> Result<(), CoreError> {
        let answer = self.call("DELETE", "/api/device/local", &[], None, CALL_TIMEOUT)?;
        if answer.status != 200 {
            return Err(failed(format!("DELETE /api/device/local answered {}", answer.status)));
        }
        Ok(())
    }

    /// Whether the core accepts `token`: an authenticated read (the device list), 200 or 401.
    pub fn accepts(&self, token: &str) -> Result<bool, CoreError> {
        let bearer = format!("Bearer {token}");
        let answer = self.call("GET", "/api/device/devices", &[("authorization", &bearer)], None, CALL_TIMEOUT)?;
        match answer.status {
            200 => Ok(true),
            401 => Ok(false),
            status => Err(failed(format!("GET /api/device/devices answered {status}"))),
        }
    }
}
