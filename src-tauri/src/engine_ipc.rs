//! The native engine, as the bundled interface reaches it (docs/BRIDGE.md → "The native engine").
//!
//! The bridge script exposes these as `window.__sidevoiceDesktop.host.engine`. Only the room window's own
//! pages may call them (capabilities/room.json); the settings window may read what is on disk
//! (`engine_on_disk`, capabilities/settings.json). They manage the models: the catalogues and what they list, the keys
//! of remote providers (kept in the keychain, never handed back), what runs here, what is installed and on disk,
//! installs and their cancel, removing a model, memory. Engine work runs on blocking threads, never the main one. A refusal is
//! the engine's keyed `Error` (docs/BRIDGE.md → "Refusals").

use sidevoice_desktop_engine::error::{self, internal};
use sidevoice_desktop_engine::{
    CatalogView, CredentialRefusal, Device, Error, Installed, Jobs, Memory, NativeEngines, OnDisk, Progress, Wanted,
};
use std::sync::Arc;
use tauri::State;

pub struct EngineState {
    pub engines: Arc<NativeEngines>,
    /// Each install in flight, by the page's job id: its progress, and its cancel.
    jobs: Arc<Jobs>,
}

impl EngineState {
    pub fn new(engines: NativeEngines) -> Self {
        EngineState { engines: Arc::new(engines), jobs: Arc::default() }
    }
}

/// `{runs: "native", os, arch, has, memory_mb}`: what the page resolves its offers from.
#[tauri::command]
pub fn engine_capabilities(state: State<'_, EngineState>) -> Device {
    state.engines.device.clone()
}

/// `[{model, engine}]`: the builds on disk.
#[tauri::command]
pub async fn engine_installed(state: State<'_, EngineState>) -> Result<Vec<Installed>, Error> {
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.installed()).await.map_err(internal)?
}

/// For the settings window: engine packages and model builds on disk, with their sizes.
#[tauri::command]
pub async fn engine_on_disk(state: State<'_, EngineState>) -> Result<OnDisk, Error> {
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.on_disk()).await.map_err(internal)?
}

/// Installs `model` as `job` (the page's id for this call): build `build` (its catalogue id), which must be `engine`'s
/// when that is named too; else the build for `engine`; else the one the catalogue recommends here. Its progress while
/// it runs, cancellable with `engine_cancel` until it ends.
#[tauri::command]
pub async fn engine_install(
    state: State<'_, EngineState>,
    model: String,
    build: Option<String>,
    engine: Option<String>,
    job: String,
) -> Result<(), Error> {
    let (engines, jobs) = (state.engines.clone(), state.jobs.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let wanted = Wanted { build: build.as_deref(), engine: engine.as_deref() };
        engines.install_job(&jobs, &job, &model, wanted)
    })
    .await
    .map_err(internal)?
}

/// `{job, model, engine, done, total, bytes_per_s}` of install `job` once it has started; `null` while it waits or
/// after it ends.
#[tauri::command]
pub fn engine_progress(state: State<'_, EngineState>, job: String) -> Option<Progress> {
    state.jobs.get(&job)
}

/// Cancels install `job`, waiting or running: it removes what it was downloading and rejects with
/// `install_cancelled`. True when the job was running or waiting here.
#[tauri::command]
pub fn engine_cancel(state: State<'_, EngineState>, job: String) -> bool {
    state.jobs.cancel(&job)
}

/// Takes `model` off this device, every build of it. Refused `model_installing` while an install job of it is waiting
/// or running, `model_in_use` while the voice call holds it, `model_unknown` for a model the catalogue does not list.
#[tauri::command]
pub async fn engine_uninstall(state: State<'_, EngineState>, model: String) -> Result<(), Error> {
    let (engines, jobs) = (state.engines.clone(), state.jobs.clone());
    tauri::async_runtime::spawn_blocking(move || engines.uninstall(&jobs, &model)).await.map_err(internal)?
}

/// `{total_mb, available_mb}`.
#[tauri::command]
pub fn engine_memory(state: State<'_, EngineState>) -> Memory {
    state.engines.memory()
}

/// `[{id, name, status, models}]`: every catalogue of the engine, the local one first, then each provider, with every
/// model it lists. One that cannot list (a provider with no key) has none, and its status says why.
#[tauri::command]
pub async fn engine_catalogs(state: State<'_, EngineState>) -> Result<Vec<CatalogView>, Error> {
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.catalogs()).await.map_err(internal)?
}

/// Keeps `key` for `provider` in the keychain, or removes it (`null` or blank), then reads that provider's catalogue
/// again with what the keychain now has.
#[tauri::command]
pub async fn engine_set_credential(
    state: State<'_, EngineState>,
    provider: String,
    key: Option<String>,
) -> Result<(), Error> {
    let key = key.as_deref().map(str::trim).filter(|key| !key.is_empty()).map(str::to_string);
    credentials::write(&provider, key.as_deref())?;
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.refresh(&provider)).await.map_err(internal)?
}

/// Asks `provider` with `key` without keeping it: never written to the keychain, never in the engine's own listings
/// (`NativeEngines::test_credential`). Answers what the provider's catalogue says against it (`{code, params, detail?}`,
/// `detail` the provider's own words without the key), or `null` when it lists with it. Refused as
/// `engine_set_credential` is where the app keeps no keys: a key it could not keep is not worth trying. The key is
/// never logged.
#[tauri::command]
pub async fn engine_test_credential(
    state: State<'_, EngineState>,
    provider: String,
    key: String,
) -> Result<Option<CredentialRefusal>, Error> {
    credentials::check(&provider)?;
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.test_credential(&provider, &key)).await.map_err(internal)?
}

/// Reads `provider`'s catalogue again now, past any listing kept, with the key the keychain has.
#[tauri::command]
pub async fn engine_refresh(state: State<'_, EngineState>, provider: String) -> Result<(), Error> {
    let engines = state.engines.clone();
    tauri::async_runtime::spawn_blocking(move || engines.refresh(&provider)).await.map_err(internal)?
}

/// Whether the keychain holds a key for `provider`. The key itself never leaves the app.
#[tauri::command]
pub fn engine_has_credential(provider: String) -> Result<bool, Error> {
    credentials::has(&provider)
}

/// Provider keys, in the keychain where the app has one (macOS).
#[cfg(target_os = "macos")]
mod credentials {
    use super::{error, Error};
    use crate::keychain;

    fn provider(provider: &str) -> Result<(), Error> {
        if keychain::valid(provider) {
            Ok(())
        } else {
            Err(error::provider_invalid(provider))
        }
    }

    /// Whether a key for `id` could be kept here.
    pub fn check(id: &str) -> Result<(), Error> {
        provider(id)
    }

    pub fn write(id: &str, key: Option<&str>) -> Result<(), Error> {
        provider(id)?;
        keychain::write(id, key).map_err(error::credentials_failed)
    }

    pub fn has(id: &str) -> Result<bool, Error> {
        provider(id)?;
        keychain::read(id).map(|key| key.is_some()).map_err(error::credentials_failed)
    }
}

/// Elsewhere the app keeps no keys: none is held, and none can be set.
#[cfg(not(target_os = "macos"))]
mod credentials {
    use super::{error, Error};

    pub fn check(_: &str) -> Result<(), Error> {
        Err(error::credentials_unavailable())
    }

    pub fn write(_: &str, _: Option<&str>) -> Result<(), Error> {
        Err(error::credentials_unavailable())
    }

    pub fn has(_: &str) -> Result<bool, Error> {
        Ok(false)
    }
}
