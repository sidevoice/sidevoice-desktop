//! The engine's commands (`window.__sidevoiceDesktop.host.engine`, docs/BRIDGE.md → "The native engine"): the catalogues
//! and what they list, the keys of remote providers (kept by the shell's [`ProviderKeys`], never handed back), what runs
//! here, what is installed, installs and their cancel, removing a model, memory. Engine work runs on blocking threads,
//! never on a worker. A refusal is the engine's keyed `Error` (docs/BRIDGE.md → "Refusals").

use std::sync::Arc;

use sidevoice_engine::{async_trait, Credentials};
use tokio::runtime::Handle;

use super::{AppHost, KeyError, Lease, ProviderKeys};
use crate::engine::error::{self, internal};
use crate::engine::{
    CatalogView, CredentialRefusal, Device, Error, Installed, Memory, NativeEngines, Progress, Wanted,
};
use crate::voice::host::LaneAudio;

/// The keys of the shell's store as the engine reads them. The engine reads the same store as the bridge.
pub(super) struct EngineKeys {
    keys: Arc<dyn ProviderKeys>,
    runtime: Handle,
}

impl EngineKeys {
    pub(super) fn new(keys: Arc<dyn ProviderKeys>, runtime: Handle) -> Self {
        EngineKeys { keys, runtime }
    }
}

#[async_trait]
impl Credentials for EngineKeys {
    async fn credential(&self, provider: &str) -> sidevoice_engine::Result<Option<String>> {
        let (keys, provider) = (Arc::clone(&self.keys), provider.to_owned());
        let result = self
            .runtime
            .spawn_blocking(move || keys.read(&provider))
            .await
            .map_err(|_| sidevoice_engine::Error::new("credentials-failed"))?;
        result.map_err(|error| {
            sidevoice_engine::Error::new(match error {
                KeyError::Unavailable => "credentials-unavailable",
                _ => "credentials-failed",
            })
        })
    }
}

/// Never the store's error text: it can contain secret bytes.
fn refusal(error: KeyError) -> Error {
    match error {
        KeyError::Unavailable => error::credentials_unavailable(),
        KeyError::TooLong => error::credentials_too_long(),
        KeyError::InvalidProvider | KeyError::Failed => error::credentials_failed(),
    }
}

fn provider(keys: &dyn ProviderKeys, id: &str) -> Result<(), Error> {
    if keys.valid(id) {
        Ok(())
    } else {
        Err(error::provider_invalid(id))
    }
}

impl<A: LaneAudio> AppHost<A> {
    fn lease(&self) -> Result<Lease, Error> {
        self.inner.hooks.admission.admit().ok_or_else(error::reset_busy)
    }

    fn engines(&self) -> Result<Arc<NativeEngines>, Error> {
        self.inner.engines.clone().ok_or_else(error::engine_unavailable)
    }

    /// `work` on a blocking thread of the runtime.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.inner.runtime.spawn_blocking(work).await.map_err(internal)?
    }

    /// `{runs: "native", os, arch, has, memory_mb}`: what the page resolves its offers from.
    pub fn capabilities(&self) -> Result<Device, Error> {
        Ok(self.engines()?.device.clone())
    }

    /// `[{model, engine}]`: the builds on disk.
    pub async fn installed(&self) -> Result<Vec<Installed>, Error> {
        let engines = self.engines()?;
        self.blocking(move || engines.installed()).await
    }

    /// Installs `model` as `job` (the page's id for this call): build `build` (its catalogue id), which must be `engine`'s
    /// when that is named too; else the build for `engine`; else the one the catalogue recommends here. Its progress
    /// while it runs, cancellable with [`AppHost::cancel`] until it ends.
    pub async fn install(
        &self,
        model: String,
        build: Option<String>,
        engine: Option<String>,
        job: String,
    ) -> Result<(), Error> {
        let lease = self.lease()?;
        let (engines, jobs) = (self.engines()?, Arc::clone(&self.inner.jobs));
        self.blocking(move || {
            let _lease = lease;
            let wanted = Wanted { build: build.as_deref(), engine: engine.as_deref() };
            engines.install_job(&jobs, &job, &model, wanted)
        })
        .await
    }

    /// `{job, model, engine, done, total, bytes_per_s}` of install `job` once it has started; `None` while it waits or
    /// after it ends.
    pub fn progress(&self, job: &str) -> Option<Progress> {
        self.inner.jobs.get(job)
    }

    /// Whether an install could still write into the shared model store: the app must not reset its data while one can.
    pub fn installing(&self) -> bool {
        self.inner.jobs.busy()
    }

    /// Cancels install `job`, waiting or running: it removes what it was downloading and rejects with
    /// `install_cancelled`. True when the job was running or waiting here.
    pub fn cancel(&self, job: &str) -> bool {
        self.inner.jobs.cancel(job)
    }

    /// Takes `model` off this device, every build of it. Refused `model_installing` while an install job of it is
    /// waiting or running, `model_in_use` while the voice call holds it, `model_unknown` for a model the catalogue does
    /// not list.
    pub async fn uninstall(&self, model: String) -> Result<(), Error> {
        let lease = self.lease()?;
        let (engines, jobs) = (self.engines()?, Arc::clone(&self.inner.jobs));
        self.blocking(move || {
            let _lease = lease;
            engines.uninstall(&jobs, &model)
        })
        .await
    }

    /// `{total_mb, available_mb}`.
    pub fn memory(&self) -> Result<Memory, Error> {
        Ok(self.engines()?.memory())
    }

    /// `[{id, name, status, models}]`: every catalogue of the engine, the local one first, then each provider, with every
    /// model it lists. One that cannot list (a provider with no key) has none, and its status says why.
    pub async fn catalogs(&self) -> Result<Vec<CatalogView>, Error> {
        let engines = self.engines()?;
        self.blocking(move || engines.catalogs()).await
    }

    /// Keeps `key` for `provider` in the store, or removes it (`None` or blank), then reads that provider's catalogue
    /// again with what the store now has.
    pub async fn set_credential(&self, provider: String, key: Option<String>) -> Result<(), Error> {
        let lease = self.lease()?;
        let key = key.as_deref().map(str::trim).filter(|key| !key.is_empty()).map(str::to_string);
        let (engines, keys) = (self.engines()?, Arc::clone(&self.inner.hooks.keys));
        self.blocking(move || {
            let _lease = lease;
            self::provider(&*keys, &provider)?;
            keys.write(&provider, key.as_deref()).map_err(refusal)?;
            engines.refresh(&provider)
        })
        .await
    }

    /// Asks `provider` with `key` without keeping it: never written to the store, never in the engine's own listings
    /// ([`NativeEngines::test_credential`]). Answers what the provider's catalogue says against it (`{code, params,
    /// detail?}`, `detail` the provider's own words without the key), or `None` when it lists with it. The key is never
    /// logged.
    pub async fn test_credential(&self, provider: String, key: String) -> Result<Option<CredentialRefusal>, Error> {
        let (engines, keys) = (self.engines()?, Arc::clone(&self.inner.hooks.keys));
        self.blocking(move || {
            // A trial uses only its supplied key. It neither reads nor unlocks the persisted credential.
            self::provider(&*keys, &provider)?;
            engines.test_credential(&provider, &key)
        })
        .await
    }

    /// Reads `provider`'s catalogue again now, past any listing kept, with the key the store has.
    pub async fn refresh(&self, provider: String) -> Result<(), Error> {
        let engines = self.engines()?;
        self.blocking(move || engines.refresh(&provider)).await
    }

    /// Whether the store holds a key for `provider`. The key itself never leaves the app.
    pub async fn has_credential(&self, provider: String) -> Result<bool, Error> {
        let keys = Arc::clone(&self.inner.hooks.keys);
        self.blocking(move || {
            self::provider(&*keys, &provider)?;
            keys.read(&provider).map(|key| key.is_some()).map_err(refusal)
        })
        .await
    }
}
