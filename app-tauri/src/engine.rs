//! The engine's commands (`window.__sidevoiceDesktop.host.engine`, docs/BRIDGE.md → "The native engine"): the catalogues and
//! what they list, the keys of remote providers (kept by the app's store, never handed back), what runs here, what is
//! installed, installs and their cancel, removing a model, memory. Each is the caller check, the arguments and one call of
//! the application host, which does the work off the main thread and refuses with the engine's keyed `Error`
//! (docs/BRIDGE.md → "Refusals").
//!
//! Only the page the app owns the calls for may call these (the capability, and the app's [`crate::init`] caller): the
//! capability cannot know which room window is the current one, so the check is what refuses a stale one (ACL.md).

use sidevoice_app_core::app::AppHost;
use sidevoice_app_core::engine::error::bad_request;
use sidevoice_app_core::engine::{CatalogView, CredentialRefusal, Device, Error, Installed, Memory, Progress};
use tauri::{Runtime, State, Webview};

use crate::Callers;

fn allowed<R: Runtime>(callers: &Callers<R>, webview: &Webview<R>) -> Result<(), Error> {
    callers.check(webview).map_err(bad_request)
}

/// `{runs: "native", os, arch, has, memory_mb}`: what the page resolves its offers from.
#[tauri::command]
pub fn engine_capabilities<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
) -> Result<Device, Error> {
    allowed(&callers, &webview)?;
    host.capabilities()
}

/// `[{model, engine}]`: the builds on disk.
#[tauri::command]
pub async fn engine_installed<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
) -> Result<Vec<Installed>, Error> {
    allowed(&callers, &webview)?;
    host.installed().await
}

/// Installs `model` as `job` (the page's id for this call): build `build` (its catalogue id), which must be `engine`'s
/// when that is named too; else the build for `engine`; else the one the catalogue recommends here. Its progress while
/// it runs, cancellable with `engine_cancel` until it ends.
#[tauri::command]
pub async fn engine_install<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    model: String,
    build: Option<String>,
    engine: Option<String>,
    job: String,
) -> Result<(), Error> {
    allowed(&callers, &webview)?;
    host.install(model, build, engine, job).await
}

/// `{job, model, engine, done, total, bytes_per_s}` of install `job` once it has started; `null` while it waits or
/// after it ends.
#[tauri::command]
pub fn engine_progress<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    job: String,
) -> Result<Option<Progress>, Error> {
    allowed(&callers, &webview)?;
    Ok(host.progress(&job))
}

/// Cancels install `job`, waiting or running: it removes what it was downloading and rejects with
/// `install_cancelled`. True when the job was running or waiting here.
#[tauri::command]
pub fn engine_cancel<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    job: String,
) -> Result<bool, Error> {
    allowed(&callers, &webview)?;
    Ok(host.cancel(&job))
}

/// Takes `model` off this device, every build of it. Refused `model_installing` while an install job of it is waiting
/// or running, `model_in_use` while the voice call holds it, `model_unknown` for a model the catalogue does not list.
#[tauri::command]
pub async fn engine_uninstall<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    model: String,
) -> Result<(), Error> {
    allowed(&callers, &webview)?;
    host.uninstall(model).await
}

/// `{total_mb, available_mb}`.
#[tauri::command]
pub fn engine_memory<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
) -> Result<Memory, Error> {
    allowed(&callers, &webview)?;
    host.memory()
}

/// `[{id, name, status, models}]`: every catalogue of the engine, the local one first, then each provider, with every
/// model it lists. One that cannot list (a provider with no key) has none, and its status says why.
#[tauri::command]
pub async fn engine_catalogs<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
) -> Result<Vec<CatalogView>, Error> {
    allowed(&callers, &webview)?;
    host.catalogs().await
}

/// Keeps `key` for `provider` in the keychain, or removes it (`null` or blank), then reads that provider's catalogue
/// again with what the keychain now has.
#[tauri::command]
pub async fn engine_set_credential<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    provider: String,
    key: Option<String>,
) -> Result<(), Error> {
    allowed(&callers, &webview)?;
    host.set_credential(provider, key).await
}

/// Asks `provider` with `key` without keeping it: never written to the keychain, never in the engine's own listings.
/// Answers what the provider's catalogue says against it (`{code, params, detail?}`, `detail` the provider's own words
/// without the key), or `null` when it lists with it. The key is never logged.
#[tauri::command]
pub async fn engine_test_credential<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    provider: String,
    key: String,
) -> Result<Option<CredentialRefusal>, Error> {
    allowed(&callers, &webview)?;
    host.test_credential(provider, key).await
}

/// Reads `provider`'s catalogue again now, past any listing kept, with the key the keychain has.
#[tauri::command]
pub async fn engine_refresh<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    provider: String,
) -> Result<(), Error> {
    allowed(&callers, &webview)?;
    host.refresh(provider).await
}

/// Whether the keychain holds a key for `provider`. The key itself never leaves the app.
#[tauri::command]
pub async fn engine_has_credential<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    host: State<'_, AppHost>,
    provider: String,
) -> Result<bool, Error> {
    allowed(&callers, &webview)?;
    host.has_credential(provider).await
}
