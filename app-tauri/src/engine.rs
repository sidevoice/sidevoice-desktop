//! The engine's commands (`window.__sidevoiceDesktop.host.engine`, docs/BRIDGE.md → "The native engine"): the catalogues and
//! what they list, the keys of remote providers (kept by the app's store, never handed back), what runs here, what is
//! installed, installs and their cancel, removing a model, memory. Each is one call of the application host, which does
//! the work off the main thread and refuses with the engine's keyed `Error` (docs/BRIDGE.md → "Refusals").
//!
//! They ask no caller beyond the capability: the window an app grants them to may call them (ACL.md).

use sidevoice_app_core::app::AppHost;
use sidevoice_app_core::engine::{CatalogView, CredentialRefusal, Device, Error, Installed, Memory, Progress};
use tauri::State;

/// `{runs: "native", os, arch, has, memory_mb}`: what the page resolves its offers from.
#[tauri::command]
pub fn engine_capabilities(host: State<'_, AppHost>) -> Result<Device, Error> {
    host.capabilities()
}

/// `[{model, engine}]`: the builds on disk.
#[tauri::command]
pub async fn engine_installed(host: State<'_, AppHost>) -> Result<Vec<Installed>, Error> {
    host.installed().await
}

/// Installs `model` as `job` (the page's id for this call): build `build` (its catalogue id), which must be `engine`'s
/// when that is named too; else the build for `engine`; else the one the catalogue recommends here. Its progress while
/// it runs, cancellable with `engine_cancel` until it ends.
#[tauri::command]
pub async fn engine_install(
    host: State<'_, AppHost>,
    model: String,
    build: Option<String>,
    engine: Option<String>,
    job: String,
) -> Result<(), Error> {
    host.install(model, build, engine, job).await
}

/// `{job, model, engine, done, total, bytes_per_s}` of install `job` once it has started; `null` while it waits or
/// after it ends.
#[tauri::command]
pub fn engine_progress(host: State<'_, AppHost>, job: String) -> Option<Progress> {
    host.progress(&job)
}

/// Cancels install `job`, waiting or running: it removes what it was downloading and rejects with
/// `install_cancelled`. True when the job was running or waiting here.
#[tauri::command]
pub fn engine_cancel(host: State<'_, AppHost>, job: String) -> bool {
    host.cancel(&job)
}

/// Takes `model` off this device, every build of it. Refused `model_installing` while an install job of it is waiting
/// or running, `model_in_use` while the voice call holds it, `model_unknown` for a model the catalogue does not list.
#[tauri::command]
pub async fn engine_uninstall(host: State<'_, AppHost>, model: String) -> Result<(), Error> {
    host.uninstall(model).await
}

/// `{total_mb, available_mb}`.
#[tauri::command]
pub fn engine_memory(host: State<'_, AppHost>) -> Result<Memory, Error> {
    host.memory()
}

/// `[{id, name, status, models}]`: every catalogue of the engine, the local one first, then each provider, with every
/// model it lists. One that cannot list (a provider with no key) has none, and its status says why.
#[tauri::command]
pub async fn engine_catalogs(host: State<'_, AppHost>) -> Result<Vec<CatalogView>, Error> {
    host.catalogs().await
}

/// Keeps `key` for `provider` in the keychain, or removes it (`null` or blank), then reads that provider's catalogue
/// again with what the keychain now has.
#[tauri::command]
pub async fn engine_set_credential(
    host: State<'_, AppHost>,
    provider: String,
    key: Option<String>,
) -> Result<(), Error> {
    host.set_credential(provider, key).await
}

/// Asks `provider` with `key` without keeping it: never written to the keychain, never in the engine's own listings.
/// Answers what the provider's catalogue says against it (`{code, params, detail?}`, `detail` the provider's own words
/// without the key), or `null` when it lists with it. The key is never logged.
#[tauri::command]
pub async fn engine_test_credential(
    host: State<'_, AppHost>,
    provider: String,
    key: String,
) -> Result<Option<CredentialRefusal>, Error> {
    host.test_credential(provider, key).await
}

/// Reads `provider`'s catalogue again now, past any listing kept, with the key the keychain has.
#[tauri::command]
pub async fn engine_refresh(host: State<'_, AppHost>, provider: String) -> Result<(), Error> {
    host.refresh(provider).await
}

/// Whether the keychain holds a key for `provider`. The key itself never leaves the app.
#[tauri::command]
pub async fn engine_has_credential(host: State<'_, AppHost>, provider: String) -> Result<bool, Error> {
    host.has_credential(provider).await
}
