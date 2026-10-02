//! The local host: the core on this computer, as the desktop app reaches it (docs/LOCAL_HOST.md).
//!
//! - [`checks`]: before any connect, the core's directory is this user's and private, and the socket's peer is this
//!   user (`getpeereid` / `SO_PEERCRED`).
//! - [`core_socket`]: a minimal HTTP/1.1 client over `C/local.sock` — health, identity, local pairing.
//! - [`identity`]: the core proves its identity by signing a nonce with the key the app pinned.
//! - [`store`]: `local-host.json` (0600) in the app's config directory: the pairing and its device token.
//! - [`connector`], [`cli`]: the node service's state (`node.status` on `D/connector.sock`) and the connector's CLI,
//!   run through `D/install.json` `command` only.
//! - [`state`]: what the bridge reports, from all of the above (pure).
//! - [`proxy`]: the page's way in — `127.0.0.1:<ephemeral>`, a per-launch secret, the Origin/Host rules — which swaps
//!   the secret for the device token and carries the request over the socket.
//! - [`host`]: the startup cases, the poll, and the actions, tied together.
//!
//! The trust boundary is the OS user: the device token travels only over the socket, and the page only ever holds
//! the proxy's in-memory secret (sidevoice design "Onboarding, hosts and settings" §1, §4.1).
//!
//! Unix only: the beta's local host is macOS (the app) and Linux (tests and headless machines); elsewhere the crate is
//! empty and the app offers no local host.

#![cfg(unix)]

pub mod checks;
pub mod cli;
pub mod connector;
pub mod core_socket;
#[cfg(feature = "fake-core")]
pub mod fake_core;
pub mod host;
pub mod http;
pub mod identity;
pub mod paths;
pub mod proxy;
pub mod state;
pub mod store;
pub mod trusted;

use serde::Serialize;
use std::fmt;

/// A refusal or failure with a stable `key` the page translates and `message`, an English sentence for a client that
/// does not know the key (the shape of the native engine's refusals, docs/BRIDGE.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub key: String,
    pub message: String,
}

impl Refusal {
    pub fn new(key: &str, message: impl Into<String>) -> Self {
        Refusal { key: key.to_string(), message: message.into() }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.message)
    }
}

impl std::error::Error for Refusal {}
