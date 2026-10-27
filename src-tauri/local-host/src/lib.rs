//! The local host: the core on this computer, as the desktop app reaches it (docs/LOCAL_HOST.md).
//!
//! - [`checks`]: before any connect, the core's directory is this user's and private; the server at the other end of
//!   the socket must be this user (`sidevoice_platform::ipc`'s connect checks it, [`platform::ipc`]).
//! - [`endpoints`]: where the core and the connector listen, read from the records they publish (`C/core.json`,
//!   `D/connector.json`), never computed.
//! - [`core_socket`]: a minimal HTTP/1.1 client over the core's socket (`C/core.json`) — health, identity, local pairing.
//! - [`identity`]: the core proves its identity by signing a nonce with the key the app pinned.
//! - [`store`]: `local-host.json` (0600) in the app's config directory: the pairing and its device token.
//! - [`connector`], [`cli`]: the node service's state (`node.status` on the socket `D/connector.json` names) and the connector's CLI,
//!   run through `D/install.json` `command` only.
//! - [`state`]: what the bridge reports, from all of the above (pure).
//! - [`proxy`]: the page's way in — `127.0.0.1:<ephemeral>`, a per-launch secret, the Origin/Host rules — which swaps
//!   the secret for the device token and carries the request over the socket.
//! - [`npm`], [`tarball`], [`install`]: preparing this computer from the app, without Node — the `sidevoice` package
//!   from the npm registry (HTTPS, pinned to the registry, its SHA-512 checked), unpacked by hand into a private
//!   directory, and its own installer run from there.
//! - [`host`]: the startup cases, the poll, the actions and the install, tied together.
//! - [`platform`]: the way to the OS. What differs per OS is `sidevoice_platform`'s (the local channel and its peer
//!   check, child process trees and their bounded output, private files and programs); `platform` only drives its
//!   async channel from blocking code. Nothing here has code of its own per OS.
//!
//! The trust boundary is the OS user: the device token travels only over the socket, and the page only ever holds
//! the proxy's in-memory secret (sidevoice design "Onboarding, hosts and settings" §1, §4.1).
//!
//! The app offers the local host on every OS. Everywhere, what `sidevoice_platform` cannot observe is refused: an
//! unknown peer is not trusted, a process that cannot be asked about is not alive.

pub mod checks;
pub mod cli;
pub mod connector;
pub mod core_socket;
pub mod endpoints;
pub mod host;
pub mod http;
pub mod identity;
pub mod install;
pub mod npm;
pub mod paths;
pub mod platform;
pub mod proxy;
mod runtime;
pub mod state;
pub mod store;
pub mod tarball;

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
