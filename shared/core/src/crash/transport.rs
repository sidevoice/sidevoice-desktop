//! What every envelope passes before it leaves, and the POST that says whether it arrived.
//!
//! The SDK's own transports queue and send in the background and never say how it went; a machine's record may be
//! acknowledged only after a successful send, so the transport here is ours: it takes an envelope on the caller's thread,
//! posts it, and records the outcome of the events somebody waits for ([`Gate::watch`], [`Gate::take_outcome`]): the
//! outcome is kept per event id, so what is posted at the same time (an app panic during a machine's record) cannot
//! answer for another event.
//!
//! The gate is the app's last word. It is open only while the mode is `auto` ([`Gate::close`] when the person chooses
//! `never`), it is checked again just before the POST, and it lets through envelopes made of events only: of whatever an
//! envelope holds, only its event goes on; no session, client report, transaction or attachment ever leaves. A closed gate drops what arrives; nothing waits in a queue, and there is
//! no disk cache.

use sentry::protocol::EnvelopeItem;
use sentry::types::{Dsn, Uuid};
use sentry::{Envelope, Transport};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// The POST failed (no network, a refusal, a rate limit, a timeout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostError;

/// How long the POST of a machine's record may take: it runs on the pass's own thread.
pub const RECORD_BUDGET: Duration = Duration::from_secs(10);
/// How long the POST of the app's own panic may take: it runs inside the panic hook, which holds the dying process up. A
/// panic report is best-effort; a slow network loses it rather than the exit.
pub const PANIC_BUDGET: Duration = Duration::from_secs(2);
/// How many POSTs may be under way at once. A send that finds them all busy is dropped, not queued: nothing waits, and a
/// stuck network cannot pile threads up.
pub const MAX_IN_FLIGHT: usize = 4;

/// Where an envelope's bytes go; the real one posts to Sentry, a test keeps them. `budget` is the most it may take.
pub trait Post: Send + Sync + 'static {
    fn post(&self, envelope: Vec<u8>, budget: Duration) -> Result<(), PostError>;
}

/// The gate: see the module's head.
pub struct Gate {
    open: AtomicBool,
    post: Arc<dyn Post>,
    in_flight: AtomicUsize,
    /// The events somebody waits for, by event id: `None` until the envelope is handled.
    watched: Mutex<HashMap<Uuid, Option<bool>>>,
}

impl Gate {
    pub fn new(post: Arc<dyn Post>) -> Gate {
        Gate { open: AtomicBool::new(true), post, in_flight: AtomicUsize::new(0), watched: Mutex::new(HashMap::new()) }
    }

    /// From now on nothing is sent, including what is being handed over this instant.
    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    /// Starts waiting for the event `id` (a machine's record: its event id is the record's id). Its POST gets the longer
    /// budget, and its outcome is kept for [`Gate::take_outcome`].
    pub fn watch(&self, id: Uuid) {
        self.watched.lock().unwrap_or_else(PoisonError::into_inner).insert(id, None);
    }

    fn record(&self, id: Uuid, outcome: bool) {
        if let Some(entry) = self.watched.lock().unwrap_or_else(PoisonError::into_inner).get_mut(&id) {
            *entry = Some(outcome);
        }
    }

    fn is_watched(&self, id: Uuid) -> bool {
        self.watched.lock().unwrap_or_else(PoisonError::into_inner).contains_key(&id)
    }

    /// How the watched event `id` went, and the end of the watch: `Some(true)` delivered, `Some(false)` the POST failed,
    /// `None` it was dropped (a closed gate, or not an event) or never arrived.
    pub fn take_outcome(&self, id: Uuid) -> Option<bool> {
        self.watched.lock().unwrap_or_else(PoisonError::into_inner).remove(&id).flatten()
    }
}

impl Transport for Gate {
    fn send_envelope(&self, envelope: Envelope) {
        // The SDK attaches its pending client reports to an outgoing envelope: only the event goes on.
        let event = envelope.items().find_map(|item| match item {
            EnvelopeItem::Event(event) => Some((**event).clone()),
            _ => None,
        });
        let Some(event) = event else { return };
        let id = event.event_id;
        if !self.is_open() {
            return;
        }
        let mut bytes = Vec::new();
        if Envelope::from(event).to_writer(&mut bytes).is_err() {
            self.record(id, false);
            return;
        }
        if !self.is_open() {
            return;
        }
        let budget = if self.is_watched(id) { RECORD_BUDGET } else { PANIC_BUDGET };
        if self.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            tracing::debug!("a crash report was dropped: sends are already under way");
            self.record(id, false);
            return;
        }
        // A transport that panics ends here: the caller's thread (the app's, or a pass's) never sees it.
        let sent = super::contained("post", || self.post.post(bytes, budget).is_ok()).unwrap_or(false);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.record(id, sent);
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

impl SentryPost {
    pub fn new(dsn: &Dsn, user_agent: &str) -> SentryPost {
        SentryPost { url: dsn.envelope_api_url().to_string(), auth: dsn.to_auth(Some(user_agent)).to_string() }
    }
}

impl Post for SentryPost {
    /// On a thread of its own: the blocking client runs a runtime, which must not start inside an async worker, and a panic
    /// hook may be running on one.
    fn post(&self, envelope: Vec<u8>, budget: Duration) -> Result<(), PostError> {
        let (url, auth) = (self.url.clone(), self.auth.clone());
        let (tell, hear) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("crash-post".into())
            .spawn(move || {
                let sent = reqwest::blocking::Client::builder()
                    .timeout(budget)
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
        match hear.recv_timeout(budget + Duration::from_millis(500)) {
            Ok(true) => Ok(()),
            _ => Err(PostError),
        }
    }
}
