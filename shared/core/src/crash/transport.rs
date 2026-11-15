//! What every envelope passes before it leaves, and the POST that says whether it arrived.
//!
//! The SDK's own transports queue and send in the background and never say how it went; a machine's record may be
//! acknowledged only after a successful send, so the transport here is ours: it takes an envelope on the caller's thread,
//! posts it, and records the outcome ([`Gate::take_outcome`]).
//!
//! The gate is the app's last word. It is open only while the mode is `auto` ([`Gate::close`] when the person chooses
//! `never`), it is checked again just before the POST, and it lets through envelopes made of events only: of whatever an
//! envelope holds, only its event goes on; no session, client report, transaction or attachment ever leaves. A closed gate drops what arrives; nothing waits in a queue, and there is
//! no disk cache.

use sentry::protocol::EnvelopeItem;
use sentry::types::Dsn;
use sentry::{Envelope, Transport};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The POST failed (no network, a refusal, a rate limit, a timeout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostError;

/// Where an envelope's bytes go; the real one posts to Sentry, a test keeps them.
pub trait Post: Send + Sync + 'static {
    fn post(&self, envelope: Vec<u8>) -> Result<(), PostError>;
}

/// The gate: see the module's head.
pub struct Gate {
    open: AtomicBool,
    post: Arc<dyn Post>,
    outcome: Mutex<Option<bool>>,
}

impl Gate {
    pub fn new(post: Arc<dyn Post>) -> Gate {
        Gate { open: AtomicBool::new(true), post, outcome: Mutex::new(None) }
    }

    /// From now on nothing is sent, including what is being handed over this instant.
    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    fn record(&self, outcome: Option<bool>) {
        *self.outcome.lock().unwrap_or_else(PoisonError::into_inner) = outcome;
    }

    /// How the last envelope went: `Some(true)` delivered, `Some(false)` the POST failed, `None` it was dropped (a closed
    /// gate, or not an event). Read once.
    pub fn take_outcome(&self) -> Option<bool> {
        self.outcome.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl Transport for Gate {
    fn send_envelope(&self, envelope: Envelope) {
        self.record(None);
        // The SDK attaches its pending client reports to an outgoing envelope: only the event goes on.
        let event = envelope.items().find_map(|item| match item {
            EnvelopeItem::Event(event) => Some((**event).clone()),
            _ => None,
        });
        let Some(event) = event else { return };
        if !self.is_open() {
            return;
        }
        let mut bytes = Vec::new();
        if Envelope::from(event).to_writer(&mut bytes).is_err() {
            self.record(Some(false));
            return;
        }
        if !self.is_open() {
            return;
        }
        self.record(Some(self.post.post(bytes).is_ok()));
    }

    fn flush(&self, _timeout: Duration) -> bool {
        true
    }
}

/// The POST to the DSN's envelope endpoint.
pub struct SentryPost {
    url: String,
    auth: String,
}

const POST_TIMEOUT: Duration = Duration::from_secs(10);

impl SentryPost {
    pub fn new(dsn: &Dsn, user_agent: &str) -> SentryPost {
        SentryPost { url: dsn.envelope_api_url().to_string(), auth: dsn.to_auth(Some(user_agent)).to_string() }
    }
}

impl Post for SentryPost {
    /// On a thread of its own: the blocking client runs a runtime, which must not start inside an async worker, and a panic
    /// hook may be running on one.
    fn post(&self, envelope: Vec<u8>) -> Result<(), PostError> {
        let (url, auth) = (self.url.clone(), self.auth.clone());
        let (tell, hear) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("crash-post".into())
            .spawn(move || {
                let sent = reqwest::blocking::Client::builder()
                    .timeout(POST_TIMEOUT)
                    .build()
                    .and_then(|client| {
                        client
                            .post(url)
                            .header("X-Sentry-Auth", auth)
                            .header("Content-Type", "application/x-sentry-envelope")
                            .body(envelope)
                            .send()
                    })
                    .is_ok_and(|answer| answer.status().is_success());
                let _ = tell.send(sent);
            })
            .map_err(|_| PostError)?;
        match hear.recv_timeout(POST_TIMEOUT + Duration::from_secs(2)) {
            Ok(true) => Ok(()),
            _ => Err(PostError),
        }
    }
}
