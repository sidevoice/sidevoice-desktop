//! A provider key tried without being kept (`NativeEngines::test_credential`).
//!
//! The trial runs on an engine of its own, made for it and dropped after it, over the app's host with one difference:
//! its only key is the one tried, for that provider alone. The app's engine never sees it: its listings, the keys a
//! call in progress hands a provider, and anything listed while the trial runs are the kept key's, before, during and
//! after, so there is nothing to restore. Nothing here prints a key (no `Debug`).

use sidevoice_engine::{async_trait, Capabilities, Credentials, Fetcher, Host, HttpClient, Storage};
use std::sync::Arc;

/// The app's host, shared by its engine and each trial's.
pub(crate) struct Shared(pub(crate) Arc<dyn Host>);

impl Host for Shared {
    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }
    fn storage(&self) -> &dyn Storage {
        self.0.storage()
    }
    fn fetcher(&self) -> &dyn Fetcher {
        self.0.fetcher()
    }
    fn http(&self) -> &dyn HttpClient {
        self.0.http()
    }
    fn credentials(&self) -> &dyn Credentials {
        self.0.credentials()
    }
}

/// The host a trial's engine runs on: the app's, with `key` as its one key, for `provider` alone (none for any other).
pub(crate) struct Trial {
    host: Arc<dyn Host>,
    key: TrialKey,
}

impl Trial {
    /// `key` trimmed; a blank one is no key (`credential-missing`).
    pub(crate) fn new(host: Arc<dyn Host>, provider: &str, key: &str) -> Self {
        let key = Some(key.trim()).filter(|key| !key.is_empty()).map(str::to_string);
        Trial { host, key: TrialKey { provider: provider.to_string(), key } }
    }
}

struct TrialKey {
    provider: String,
    key: Option<String>,
}

#[async_trait]
impl Credentials for TrialKey {
    async fn credential(&self, provider: &str) -> sidevoice_engine::Result<Option<String>> {
        Ok(if provider == self.provider { self.key.clone() } else { None })
    }
}

impl Host for Trial {
    fn capabilities(&self) -> Capabilities {
        self.host.capabilities()
    }
    fn storage(&self) -> &dyn Storage {
        self.host.storage()
    }
    fn fetcher(&self) -> &dyn Fetcher {
        self.host.fetcher()
    }
    fn http(&self) -> &dyn HttpClient {
        self.host.http()
    }
    fn credentials(&self) -> &dyn Credentials {
        &self.key
    }
}
