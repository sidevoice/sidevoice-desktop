//! The app's private pairing file, the machine's shared model root, and the confirmed reset.
//! Only the current room page can ask; no bridge method accepts a filesystem path.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Manager, Webview};

use crate::platform::reset_handoff::{self, Handoff};
use crate::platform::{account, app_data, application, connector_service, credentials, uninstall};
use crate::AppState;

#[derive(Default)]
pub(crate) struct Admission {
    active: usize,
    resetting: bool,
}

impl Admission {
    fn enter(&mut self) -> bool {
        if self.resetting {
            return false;
        }
        self.active += 1;
        true
    }
    fn leave(&mut self) {
        self.active -= 1;
    }
    fn begin_reset(&mut self) -> bool {
        if self.resetting || self.active != 0 {
            return false;
        }
        self.resetting = true;
        true
    }
    fn end_reset(&mut self) {
        self.resetting = false;
    }
}

pub(crate) struct Permit(AppHandle);

impl Drop for Permit {
    fn drop(&mut self) {
        let state = self.0.state::<AppState>();
        let mut gate = state.reset_admission.lock().unwrap();
        gate.leave();
    }
}

pub(crate) fn admit(app: &AppHandle) -> Option<Permit> {
    let state = app.state::<AppState>();
    let mut gate = state.reset_admission.lock().unwrap();
    if !gate.enter() {
        return None;
    }
    Some(Permit(app.clone()))
}

fn begin_exclusive(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut gate = state.reset_admission.lock().unwrap();
    if !gate.begin_reset() {
        return false;
    }
    state.resetting.store(true, Ordering::SeqCst);
    true
}

fn end_exclusive(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut gate = state.reset_admission.lock().unwrap();
    gate.end_reset();
    state.resetting.store(false, Ordering::SeqCst);
}

type Answer = Result<Value, Value>;

fn refused(key: &str) -> Value {
    json!({ "key": key })
}

fn retained_agents(agents: &Value) -> Vec<String> {
    agents
        .as_object()
        .map(|rows| {
            rows.iter()
                .filter_map(|(name, value)| {
                    let outcome = value.as_str().or_else(|| value.get("outcome").and_then(Value::as_str));
                    (!matches!(outcome, Some("removed" | "absent"))).then(|| name.clone())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), Value> {
    crate::room_page(app, webview).map_err(|_| refused("reset.unauthorized"))
}

pub(crate) fn desktop_dir(app: &AppHandle) -> Result<PathBuf, Value> {
    app.path().app_data_dir().map_err(|_| refused("reset.unavailable"))
}

/// The models folder the engine and the app share: `Models` by the one rule of `sidevoice-platform` (always the OS standard
/// place, the same on every OS, independent of `$SIDEVOICE_DATA_DIR`). `None` when this user has none.
pub fn shared_model_root() -> Option<PathBuf> {
    sidevoice_platform::data_dir::models_dir()
}

/// The remote pairings, in the app data folder (docs/PAIRING_STORAGE.md).
pub(crate) const PAIRINGS_FILE: &str = "pairings.json";

fn pairings_file(app: &AppHandle) -> Result<PathBuf, Value> {
    Ok(desktop_dir(app)?.join(PAIRINGS_FILE))
}

mod pairings;
use pairings::{read_pairings, write_pairings};

#[tauri::command]
pub fn data_pairings_read(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    read_pairings(&pairings_file(&app)?)
}

#[tauri::command]
pub fn data_pairings_write(app: AppHandle, webview: Webview, value: Value) -> Answer {
    caller(&app, &webview)?;
    write_pairings(&pairings_file(&app)?, &value)
}

#[tauri::command]
pub fn data_reset_report(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    let dir = control_dir().ok_or_else(|| refused("reset.unavailable"))?;
    consume_report_at(&dir)
}

/// The report, or else the journal of an interrupted reset, read once from the control directory `dir` and removed.
/// Every read and removal goes through the directory's held, checked handle (`sidevoice-platform`), in a private
/// directory. A record read must be a private regular file with one name (else `reset.unsafe`, kept) within its bound
/// (else `reset.failed` for the report, `reset.report-failed` for the journal, kept). Once a valid report is read,
/// the journal beside it is obsolete and removed unread: any regular file with one name that others cannot write,
/// never a link or reparse point.
fn consume_report_at(dir: &Path) -> Answer {
    use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
    use std::io::ErrorKind;
    let refusal = |error: std::io::Error, otherwise| {
        refused(if error.kind() == ErrorKind::PermissionDenied { "reset.unsafe" } else { otherwise })
    };
    let held = HeldDirectory::open(dir, Access::Private).map_err(|error| refusal(error, "reset.failed"))?;
    let read = |name: &str| held.read(name.as_ref(), Access::Private, RECORD_LIMIT);
    match read(REPORT).map_err(|error| refusal(error, "reset.failed"))? {
        Some(bytes) => {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.failed"))?;
            held.remove(JOURNAL.as_ref()).map_err(|error| refusal(error, "reset.report-failed"))?;
            held.remove(REPORT.as_ref()).map_err(|error| refusal(error, "reset.failed"))?;
            Ok(value)
        }
        None => match read(JOURNAL).map_err(|error| refusal(error, "reset.report-failed"))? {
            Some(bytes) => {
                let progress: Value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.report-failed"))?;
                let mut failed = progress.get("failed").and_then(Value::as_array).cloned().unwrap_or_default();
                failed.push(json!({"step":"helper", "key":"reset.interrupted"}));
                held.remove(JOURNAL.as_ref()).map_err(|error| refusal(error, "reset.report-failed"))?;
                Ok(
                    json!({"state":"partial", "completed":progress.get("completed").cloned().unwrap_or_else(|| json!([])), "failed":failed, "agents":{}}),
                )
            }
            None => Ok(Value::Null),
        },
    }
}

#[tauri::command]
pub async fn data_delete_all(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    confirmed_reset(app, false).await
}

/// `deleteAll()` and then the app bundle: the helper moves it to the Trash after the app quits and does not relaunch.
#[tauri::command]
pub async fn data_uninstall(app: AppHandle, webview: Webview) -> Answer {
    caller(&app, &webview)?;
    confirmed_reset(app, true).await
}

/// The confirmed reset, on every host: the checks and the native confirmation come before any data is touched.
async fn confirmed_reset(app: AppHandle, uninstall: bool) -> Answer {
    use crate::platform::confirmation::{self, ConfirmationKind};
    use crate::platform::webview_data;
    let state = app.state::<AppState>();
    if !begin_exclusive(&app) {
        return Err(refused("reset.busy"));
    }
    if let Err(error) = webview_data::preflight(std::process::id()) {
        end_exclusive(&app);
        // Existing translated guidance: open the installed app from Applications, then retry.
        let key = if error.kind() == std::io::ErrorKind::Unsupported {
            "reset.app-unremovable"
        } else {
            "reset.helper-failed"
        };
        return Err(refused(key));
    }
    if state.call.lock().unwrap().joined
        || crate::updater::installing(&app)
        || app.try_state::<crate::engine_ipc::EngineState>().is_some_and(|engine| engine.jobs.busy())
    {
        end_exclusive(&app);
        return Err(refused("reset.busy"));
    }
    if let Err(key) = refuse_before_confirmation(&app) {
        end_exclusive(&app);
        return Err(refused(key));
    }
    // A host that cannot remove the app refuses before the data goes.
    if uninstall && !uninstall::available() {
        end_exclusive(&app);
        return Err(refused("reset.app-unremovable"));
    }
    if let Err(refusal) = require_confirmation(
        confirmation::confirm(
            &app,
            if uninstall { ConfirmationKind::Uninstall } else { ConfirmationKind::DeleteAllData },
        )
        .await,
    ) {
        end_exclusive(&app);
        return Err(refusal);
    }
    let app_for_work = app.clone();
    let answer = tauri::async_runtime::spawn_blocking(move || begin_reset(&app_for_work, uninstall))
        .await
        .unwrap_or_else(|_| Err(refused("reset.failed")));
    if answer.is_err() {
        end_exclusive(&app);
    }
    answer
}

/// The transaction's own refusal before the confirmation and before any data is touched: a connector install
/// in progress.
fn refuse_before_confirmation(app: &AppHandle) -> Result<(), &'static str> {
    if crate::local_host::installing(app) {
        return Err("reset.busy");
    }
    Ok(())
}

fn require_confirmation(accepted: bool) -> Result<(), Value> {
    accepted.then_some(()).ok_or_else(|| refused("reset.cancelled"))
}

fn begin_reset(app: &AppHandle, uninstall: bool) -> Answer {
    use crate::platform::reset_handoff::ResetPlan;
    let admission = Handoff::new().and_then(|handoff| handoff.admit()).map_err(|error| {
        refused(if error.kind() == std::io::ErrorKind::WouldBlock { "reset.busy" } else { "reset.helper-failed" })
    })?;
    // The program the helper runs and the relaunch starts, resolved and checked before anything is touched.
    let program = application::installed_program().ok_or_else(|| refused("reset.helper-failed"))?;
    let (mut failed, agents, service_outcome) = reset_before_exit()?;
    // The provider keys, on every host that runs the transaction, journalled as the last step before exit.
    if credentials::clear_all().is_err() {
        failed.push(json!({"step": "keychain", "key": "reset.keychain-failed"}));
    }
    let journal = control_dir().ok_or_else(|| refused("reset.helper-failed"))?.join(JOURNAL);
    write_atomic_json(
        &journal,
        &json!({"stage":"keychain", "completed":["service", "keychain"], "failed":failed.clone()}),
    )
    .map_err(|_| refused("reset.report-failed"))?;
    let pending = reset_handoff::launch(
        admission,
        ResetPlan { parent_pid: std::process::id(), uninstall, initial: json!({"failed": failed, "agents": agents}) },
        &program,
    )
    .map_err(|_| refused("reset.helper-failed"))?;
    *app.state::<AppState>().reset_helper.lock().unwrap() = Some(pending);
    let app_to_quit = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        app_to_quit.exit(0);
    });
    let mut pending = vec!["app-data", "models", "webview", "cache", "saved-state", "logs", "preferences"];
    if uninstall {
        pending.push("app");
    }
    let state = match (failed.is_empty(), uninstall) {
        (false, _) => "partial",
        (true, false) => "needs-restart",
        (true, true) => "needs-quit",
    };
    Ok(json!({"state": state, "service": service_outcome, "agents": agents, "failed": failed, "pending": pending}))
}

/// The steps before the app exits, under the held admission: the connector service and its agents, journalled;
/// `begin_reset` then clears the provider keys. Returns the failures so far, the agents' results and the service
/// outcome. The same on every host: `D` by `sidevoice-platform`'s one rule, which must be its standard place.
fn reset_before_exit() -> Result<(Vec<Value>, Value, &'static str), Value> {
    use crate::local_host::{self, NoConnectorCli};
    use sidevoice_platform::data_dir;
    // `D` as every Sidevoice program resolves it (`SIDEVOICE_DATA_DIR`, else the standard place); the scope check
    // accepts only the standard place.
    let data = data_dir::resolve(|name| std::env::var_os(name)).map_err(|_| refused("reset.unavailable"))?;
    let standard = data_dir::standard().ok_or_else(|| refused("reset.unavailable"))?;
    // `HOME` only when it is this account's home in the account database: no other home is reset.
    let home = account::home().map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    validate_uninstall_scope_at(&home, &standard, &data)
        .map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    let installed_cli = match local_host::connector_cli(&data) {
        Ok(cli) => {
            probe_safe_connector(&cli, Duration::from_secs(10))?;
            Some(cli)
        }
        // No install record: the connector step runs only for an installed connector, so it is skipped when the
        // service is proven absent. The proof is the OS's fresh answer (`connector_service`: the service manager's
        // jobs and the running programs), never the missing record itself; and an interrupted uninstall may have
        // left data behind. Any doubt refuses before anything is touched.
        Err(_)
            if fs::symlink_metadata(data.join(INSTALL_RECORD))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            if !connector_service::absent(&home) || residual_connector_data(&data) {
                return Err(refused("reset.service-failed"));
            }
            None
        }
        Err(NoConnectorCli::Refused) => return Err(refused("reset.unsafe")),
    };
    let control = control_dir().ok_or_else(|| refused("reset.helper-failed"))?;
    let report = control.join(REPORT);
    if let Err(error) = fs::remove_file(&report) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(refused("reset.report-failed"));
        }
    }
    let journal = control.join(JOURNAL);
    write_atomic_json(&journal, &json!({"stage":"preflight", "completed":[], "failed":[]}))
        .map_err(|_| refused("reset.helper-failed"))?;
    let (service, service_outcome) = match installed_cli {
        Some(cli) => (
            run_checked_uninstall(&home, &standard, &data, || {
                local_host::run_connector(&cli, &["uninstall", "--json"], Duration::from_secs(180))
                    .ok_or_else(|| refused("reset.service-failed"))
            })?,
            "removed",
        ),
        None => (json!({"state": "absent", "agents": {}}), "absent"),
    };
    write_atomic_json(&journal, &json!({"stage":"service", "completed":["service"], "failed":[]}))
        .map_err(|_| refused("reset.report-failed"))?;
    let mut failed = Vec::<Value>::new();
    let agents = service.get("agents").cloned().unwrap_or_else(|| json!({}));
    for agent in retained_agents(&agents) {
        failed.push(json!({"step": "agents", "key": "reset.agent-remains", "agent": agent}));
    }
    write_atomic_json(&journal, &json!({"stage":"agent-results", "completed":["service"], "failed":failed.clone()}))
        .map_err(|_| refused("reset.report-failed"))?;
    Ok((failed, agents, service_outcome))
}

/// The connector's install record in `D`: the active installation and the command that runs its CLI.
const INSTALL_RECORD: &str = "install.json";

fn require_safe_uninstall(version: &Value) -> Result<(), Value> {
    if version.get("ok") == Some(&Value::Bool(true))
        && version.get("safe_uninstall").and_then(Value::as_u64).is_some_and(|level| level >= 1)
    {
        Ok(())
    } else {
        Err(refused("reset.unsupported-connector"))
    }
}

fn probe_safe_connector(cli: &crate::local_host::ConnectorCli, timeout: Duration) -> Result<(), Value> {
    let version = crate::local_host::run_connector(cli, &["--version", "--json"], timeout)
        .ok_or_else(|| refused("reset.unsupported-connector"))?;
    require_safe_uninstall(&version)
}

fn residual_connector_data(data: &Path) -> bool {
    // A missing record is not proof that an interrupted uninstall finished its data phase: the releases (`D/install`) are
    // an entry of `D` too.
    let entries = match fs::read_dir(data) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let models = sidevoice_platform::data_dir::MODELS;
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let name = entry.file_name();
        // `Models` is the engine's (docs/ENGINES.md), in the standard `D` the scope check requires: not the
        // connector's to leave behind.
        if name == models {
            continue;
        }
        if !["install.lock", "connector.lock", "agents.lock", "node-stopped.json"].iter().any(|kept| name == *kept) {
            return true;
        }
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            return true;
        }
    }
    false
}

/// The journal or report: bounded JSON published whole or not at all by the shared private-record writer (staged
/// private, synced, then renamed into the private control directory; links never followed).
fn write_atomic_json(path: &Path, value: &Value) -> Result<(), &'static str> {
    use sidevoice_platform::fs::{Files, Fs};
    Files::write_private(path, &record(value)?).map_err(|_| "report-write")
}

/// A journal or report as written: JSON within [`RECORD_LIMIT`].
fn record(value: &Value) -> Result<Vec<u8>, &'static str> {
    let bytes = serde_json::to_vec(value).map_err(|_| "report-write")?;
    if bytes.len() as u64 > RECORD_LIMIT {
        return Err("report-write");
    }
    Ok(bytes)
}

fn run_checked_uninstall<T>(
    home: &Path,
    standard: &Path,
    data: &Path,
    invoke: impl FnOnce() -> Result<T, Value>,
) -> Result<T, Value> {
    // Still this account's home in the account database, right before the destructive call.
    if account::home().as_deref() != Ok(home) {
        return Err(json!({"key": "reset.unsafe", "reason": "unsafe-home"}));
    }
    validate_uninstall_scope_at(home, standard, data)
        .map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    invoke()
}

/// The connector's data `data` is the one the reset may uninstall: the standard `D` (`standard`,
/// `sidevoice-platform`'s `data_dir::standard`, the same rule on every OS) inside the account home `home`, with the
/// releases `D/install` in it. An override (`SIDEVOICE_DATA_DIR`) is not a known directory, nor is a standard place
/// outside the home. Returns the releases directory.
fn validate_uninstall_scope_at(home: &Path, standard: &Path, data: &Path) -> Result<PathBuf, &'static str> {
    if !clean_absolute(home) || dunce::canonicalize(home).ok().as_deref() != Some(home) {
        return Err("unsafe-home");
    }
    if data != standard || !clean_absolute(data) || !data.starts_with(home) || data == home {
        return Err("outside-known-dirs");
    }
    let expected_releases = sidevoice_platform::data_dir::releases_dir(standard);
    let record = data.join(INSTALL_RECORD);
    if let Ok(meta) = fs::symlink_metadata(&record) {
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 64 * 1024 {
            return Err("unsafe-record");
        }
        let bytes = fs::read(&record).map_err(|_| "unsafe-record")?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "unsafe-record")?;
        let releases = value.get("releases").and_then(Value::as_str).ok_or("unsafe-record")?;
        if Path::new(releases) != expected_releases || !clean_absolute(Path::new(releases)) {
            return Err("outside-known-dirs");
        }
        let command = value.get("command").and_then(Value::as_array).ok_or("unsafe-record")?;
        let recovery = standard.join(".uninstall-recovery").join(connector_program());
        if command.len() != 1
            || !matches!(command[0].as_str(), Some(path) if is_release_binary(Path::new(path), &expected_releases) || Path::new(path) == recovery)
        {
            return Err("unsafe-record");
        }
    } else if fs::symlink_metadata(&record).is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound) {
        return Err("unsafe-record");
    }
    // Protected roots, not selected entries: each must be an ordinary directory, never a link.
    app_data::check_tree(home, data)?;
    app_data::check_tree(home, &expected_releases)?;
    Ok(expected_releases)
}

/// The connector's executable file name: `sidevoice-connector`, with this OS's executable suffix (`.exe` on Windows).
fn connector_program() -> String {
    format!("sidevoice-connector{}", std::env::consts::EXE_SUFFIX)
}

/// `R/releases/<id>/bin/sidevoice-connector` ([`connector_program`]): the command the connector's `install` records
/// for the release it selects, `<id>` one plain name.
fn is_release_binary(path: &Path, releases: &Path) -> bool {
    use std::path::Component;
    let Ok(rest) = path.strip_prefix(releases.join("releases")) else { return false };
    let parts: Vec<_> = rest.components().collect();
    matches!(parts.as_slice(), [Component::Normal(id), Component::Normal(bin), Component::Normal(name)]
        if !id.to_string_lossy().starts_with('.') && *bin == "bin" && *name == connector_program().as_str())
}

/// The report and the journal sit in the handoff's private control directory, outside every reset deletion root,
/// made private (and checked with everything above it) by the shared private-directory API on every OS.
fn control_dir() -> Option<PathBuf> {
    use sidevoice_platform::fs::{Files, Fs};
    let dir = reset_handoff::state_dir().ok()?;
    Files::ensure_private_dir(&dir).ok()?;
    Some(dir)
}

/// The report the page reads once: what the reset did.
const REPORT: &str = "report.json";
/// The journal: what the reset is doing, written before each step's effect and before the helper deletes anything.
const JOURNAL: &str = "journal.json";
/// Both records are bounded JSON.
const RECORD_LIMIT: u64 = 16 * 1024;
/// The environment marker of the relaunch after a reset: that app's setup finalizes the report.
const RELAUNCH_MARKER: &str = "SIDEVOICE_RESET_RELAUNCH";

/// Absolute, with no `.` or `..`: a drive or share prefix, the root, then plain names.
fn clean_absolute(path: &Path) -> bool {
    use std::path::Component;
    path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir | Component::Normal(_)))
}

/// Called in the helper process, before Tauri. The authenticated plan arrives on every host.
pub fn run_reset_helper() {
    let Ok(helper) = Handoff::new().and_then(|handoff| handoff.receive()) else { return };
    let (parent_pid, uninstall, initial) = (helper.plan.parent_pid, helper.plan.uninstall, helper.plan.initial.clone());
    clean_up_after_exit(helper, parent_pid, uninstall, initial);
}

/// The helper transaction: observe the browser writers, await the committed exit, delete the app's data
/// (`app_data`, journalled step by step), then conclude (report, relaunch or remove the app).
fn clean_up_after_exit(
    helper: crate::platform::reset_handoff::Helper,
    parent_pid: u32,
    uninstall: bool,
    initial: Value,
) {
    use crate::platform::webview_data;
    let unsafe_home = |reason| std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason);
    let browser = account::home()
        .map_err(unsafe_home)
        .and_then(|home| webview_data::locations(&home))
        .and_then(|paths| webview_data::capture(parent_pid, paths));
    let Ok(lock) = helper.await_exit() else { return };
    let Some(control) = control_dir() else { return };
    let report = control.join(REPORT);
    let Some(initial_failed) = initial.get("failed").and_then(Value::as_array) else { return };
    let mut completed = Vec::new();
    let mut failed = initial_failed.clone();
    let journal = control.join(JOURNAL);
    // The intent first: nothing is deleted before the journal write saying the cleanup has started has returned. If it
    // fails, nothing is deleted, and the journal written before the exit reports the reset as interrupted. This covers
    // a crash of the helper; whether the record also survives a power loss is the shared writer's guarantee (Unix
    // syncs the file and the directory; Windows does not yet make the rename durable: docs/RESET_HANDOFF.md).
    if write_atomic_json(&journal, &json!({"stage":"cleanup", "completed":[], "failed":failed})).is_err() {
        return;
    }
    // The carry-over from the previous identifier ends with this reset, whether it had finished or not: nothing the
    // reset deletes may come back from the old identifier's folders. Not recorded, nothing is deleted.
    if crate::legacy_identifier::stop(&control).is_err() {
        return;
    }
    let browser = browser
        .and_then(|owner| webview_data::wait_released(owner, std::time::Instant::now() + Duration::from_secs(10)));
    // A journal that cannot be written stops the deletion: an interruption could no longer be reported.
    let mut step = |step: &'static str, result: Result<(), &'static str>| {
        match result {
            Ok(()) => completed.push(step),
            Err(key) => failed.push(json!({"step": step, "key": key})),
        }
        let written = write_atomic_json(&journal, &json!({"stage":"cleanup", "completed":completed, "failed":failed}));
        if written.is_err() {
            failed.push(json!({"step":"report", "key":"reset.report-failed"}));
        }
        written.is_ok()
    };
    // Still this account's home in the account record, right before the deletion; no other home is deleted.
    let _ = match account::home() {
        Ok(home) => app_data::delete_data(&home, &browser, &mut step).is_ok(),
        Err(key) => step("app-data", Err(key)),
    };
    conclude_reset(lock, &report, &journal, completed, failed, &initial, uninstall);
}

/// The end of the transaction: the report with a relaunch, or the removal of the app where it can be restored from
/// (`uninstall::uninstall`). The decisions stay here.
fn conclude_reset(
    lock: crate::platform::reset_handoff::Admission,
    report: &Path,
    journal: &Path,
    completed: Vec<&str>,
    failed: Vec<Value>,
    initial: &Value,
    uninstall: bool,
) {
    let agents = initial.get("agents").cloned().unwrap_or_else(|| json!({}));
    let relaunch = || {
        // Cleanup is over. A new app must acquire this lock independently.
        drop(lock);
        // Resolved and checked again right before it starts.
        application::installed_program()
            .is_some_and(|program| reset_handoff::relaunch(&program, RELAUNCH_MARKER).is_ok())
    };
    if uninstall {
        let trash = || match uninstall::uninstall() {
            Ok(()) => Ok(()),
            Err(app_data::Error::Failed(key)) => Err(key),
            Err(_) => Err("reset.app-unremovable"),
        };
        finish_uninstall_at(report, journal, &completed, failed, agents, write_atomic_json, trash, relaunch);
    } else {
        finish_reset_at(report, journal, &completed, failed, agents, write_atomic_json, relaunch);
    }
}

fn finish_reset_at(
    report: &Path,
    journal: &Path,
    completed: &[&str],
    mut failed: Vec<Value>,
    agents: Value,
    write: impl Fn(&Path, &Value) -> Result<(), &'static str>,
    relaunch: impl FnOnce() -> bool,
) {
    // A successful spawn is not proof that the new app finished startup. Its setup code
    // publishes the final report; until then the journal is an interrupted partial result.
    let pending = json!({"stage":"awaiting-relaunch", "completed":completed, "failed":failed, "agents":agents});
    if write(journal, &pending).is_err() {
        return;
    }
    if !relaunch() {
        failed.push(json!({"step":"relaunch", "key":"reset.relaunch-failed"}));
        let value = json!({"state":"partial", "completed":completed, "failed":failed, "agents":agents});
        if write(report, &value).is_ok() {
            let _ = fs::remove_file(journal);
        }
    }
}

/// The uninstall's last step. Only a clean run moves the app to the Trash, and it leaves no report: nothing is left
/// to read it. A failed data step (anything but a retained agent registration, which the confirmation already names)
/// keeps the app, and so does a failed move: the app relaunches, as after `deleteAll()`, and reports it.
#[allow(clippy::too_many_arguments)]
fn finish_uninstall_at(
    report: &Path,
    journal: &Path,
    completed: &[&str],
    mut failed: Vec<Value>,
    agents: Value,
    write: impl Fn(&Path, &Value) -> Result<(), &'static str>,
    trash: impl FnOnce() -> Result<(), &'static str>,
    relaunch: impl FnOnce() -> bool,
) {
    if failed.iter().any(|row| row["step"] != "agents") {
        failed.push(json!({"step":"app", "key":"reset.app-kept"}));
    } else if write(journal, &json!({"stage":"app", "completed":completed, "failed":failed, "agents":agents})).is_err()
    {
        failed.push(json!({"step":"report", "key":"reset.report-failed"}));
        failed.push(json!({"step":"app", "key":"reset.app-kept"}));
    } else {
        match trash() {
            Ok(()) => {
                let _ = fs::remove_file(journal);
                let _ = fs::remove_file(report);
                return;
            }
            Err(key) => failed.push(json!({"step":"app", "key":key})),
        }
    }
    finish_reset_at(report, journal, completed, failed, agents, write, relaunch);
}

pub fn finalize_relaunched_reset() {
    if std::env::var(RELAUNCH_MARKER).as_deref() != Ok("1") {
        return;
    }
    let Some(control) = control_dir() else { return };
    let _ = finalize_relaunched_reset_at(&control);
}

/// The relaunched app's setup turns the helper's `awaiting-relaunch` journal in the control directory `dir` into the
/// final report: read, written and removed through the directory's held, checked handle, as the page's read is.
fn finalize_relaunched_reset_at(dir: &Path) -> Result<(), &'static str> {
    use sidevoice_platform::fs::{Access, Directory, HeldDirectory};
    let held = HeldDirectory::open(dir, Access::Private).map_err(|_| "report-read")?;
    let bytes =
        held.read(JOURNAL.as_ref(), Access::Private, RECORD_LIMIT).map_err(|_| "report-read")?.ok_or("report-read")?;
    let pending: Value = serde_json::from_slice(&bytes).map_err(|_| "report-read")?;
    if pending["stage"] != "awaiting-relaunch" {
        return Err("report-stage");
    }
    let failed = pending.get("failed").and_then(Value::as_array).ok_or("report-stage")?;
    let value = json!({"state":if failed.is_empty() {"complete"} else {"partial"},
        "completed":pending["completed"],"failed":failed,"agents":pending["agents"]});
    held.write(REPORT.as_ref(), &record(&value)?).map_err(|_| "report-write")?;
    held.remove(JOURNAL.as_ref()).map_err(|_| "report-remove")?;
    Ok(())
}

/// The transaction's decisions, on every host.
#[cfg(test)]
mod tests {
    use super::*;
    use sidevoice_platform::fs::{Files, Fs, TestFiles};

    /// A private, canonical scratch folder (as `app_data`'s tests make it) and an account home inside it.
    pub(super) fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::Builder::new().tempdir_in(dunce::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        Files::shut_others_out(temp.path());
        let home = dunce::canonicalize(temp.path()).unwrap().join("home");
        Files::create_private_dir(&home).unwrap();
        (temp, home)
    }

    /// `path` and every missing folder between `home` and it, private.
    pub(super) fn private_dirs(home: &Path, path: &Path) {
        for dir in path.ancestors().collect::<Vec<_>>().into_iter().rev().filter(|dir| dir.starts_with(home)) {
            if !dir.exists() {
                Files::create_private_dir(dir).unwrap();
            }
        }
    }

    /// The standard `D` of a fixture home, as `data_dir::standard` names it on any OS: only its place in the home
    /// matters to the scope check.
    pub(super) fn standard(home: &Path) -> PathBuf {
        home.join("standard").join("Sidevoice")
    }

    #[test]
    fn connector_uninstall_scope_refuses_overrides_before_fake_cli_runs() {
        let (temp, home) = fixture();
        let data = standard(&home);
        let mut invoked = false;
        for elsewhere in
            [home.clone(), PathBuf::from("/"), home.join("../other"), home.join("Documents"), PathBuf::from("relative")]
        {
            assert!(run_checked_uninstall(&home, &data, &elsewhere, || {
                invoked = true;
                Ok(())
            })
            .is_err());
        }
        assert!(!invoked);
        assert_eq!(
            validate_uninstall_scope_at(&home, &data, &home.join(".sidevoice")),
            Err("outside-known-dirs"),
            "the old directory is not known"
        );
        // A standard place outside the account home (an `XDG_DATA_HOME` elsewhere) is not known either.
        let outside = temp.path().join("elsewhere").join("Sidevoice");
        assert_eq!(validate_uninstall_scope_at(&home, &outside, &outside), Err("outside-known-dirs"));
        assert_eq!(validate_uninstall_scope_at(&home, &home, &home), Err("outside-known-dirs"));
        private_dirs(&home, &data);
        fs::write(data.join("install.json"), json!({"releases": home.join("Documents/sidevoice")}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data, &data), Err("outside-known-dirs"));
        let releases = data.join("install");
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [releases.join("releases").join("1.2.3").join("bin").join(connector_program())]})
                .to_string(),
        )
        .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data, &data), Ok(releases.clone()));
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [data.join(".uninstall-recovery").join(connector_program())]})
                .to_string(),
        )
        .unwrap();
        assert!(validate_uninstall_scope_at(&home, &data, &data).is_ok());
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [temp.path().join("other-program")]}).to_string(),
        )
        .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data, &data), Err("unsafe-record"));
        // Not the layout the connector records: `current` is a link of the old installer, not a release.
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [releases.join("current").join("bin").join(connector_program())]})
                .to_string(),
        )
        .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data, &data), Err("unsafe-record"));
        // A linked `D` is refused, never traversed.
        fs::remove_file(data.join("install.json")).unwrap();
        let linked = standard(&home.join("linked"));
        private_dirs(&home, linked.parent().unwrap());
        Files::symlink(&data, &linked);
        assert!(validate_uninstall_scope_at(&home, &linked, &linked).is_err());
    }

    #[test]
    fn installed_connector_must_advertise_safe_uninstall_before_reset() {
        for value in [
            json!({"ok":true,"version":"0.6.0"}),
            json!({"ok":true,"safe_uninstall":0}),
            json!({"ok":true,"safe_uninstall":"1"}),
            json!({"ok":true,"safe_uninstall":1.0}),
            json!({"ok":false,"safe_uninstall":1}),
        ] {
            assert_eq!(require_safe_uninstall(&value), Err(refused("reset.unsupported-connector")));
        }
        assert_eq!(require_safe_uninstall(&json!({"ok":true,"safe_uninstall":1})), Ok(()));
        assert_eq!(require_safe_uninstall(&json!({"ok":true,"safe_uninstall":2})), Ok(()));
    }

    #[test]
    fn retained_agent_outcomes_are_reported() {
        let agents = json!({"claude": "removed", "codex": {"outcome": "foreign"}, "cursor": {"outcome": "failed"}});
        assert_eq!(retained_agents(&agents), ["codex", "cursor"]);
    }

    #[test]
    fn direct_command_requires_native_confirmation() {
        assert_eq!(require_confirmation(false), Err(refused("reset.cancelled")));
        assert_eq!(require_confirmation(true), Ok(()));
    }

    #[test]
    fn reset_and_model_or_credential_admission_are_atomic() {
        let mut gate = Admission::default();
        assert!(gate.enter()); // model install, voice start, or credential write
        assert!(!gate.begin_reset());
        gate.leave();
        assert!(gate.begin_reset());
        assert!(!gate.enter());
        gate.end_reset();
        assert!(gate.enter());
        gate.leave();
    }

    /// The report's directory as `control_dir()` makes it: private, as the shared record writer requires.
    fn private_dir() -> (tempfile::TempDir, PathBuf) {
        let (temp, home) = fixture();
        let dir = home.join("control");
        Files::create_private_dir(&dir).unwrap();
        (temp, dir)
    }

    #[test]
    fn durable_journal_reports_interruption_then_allows_retry() {
        let (temp, dir) = private_dir();
        let report = dir.join("report.json");
        let journal = dir.join("journal.json");
        write_atomic_json(&journal, &json!({"stage":"service", "completed":["service"], "failed":[]})).unwrap();
        // Readable as a private record: this user's, not readable or writable by others.
        assert!(Files::read_private(&journal, 16 * 1024).unwrap().is_some());
        let interrupted = consume_report_at(&dir).unwrap();
        assert_eq!(interrupted["state"], "partial");
        assert_eq!(interrupted["completed"], json!(["service"]));
        assert_eq!(interrupted["failed"][0]["key"], "reset.interrupted");
        assert_eq!(consume_report_at(&dir), Ok(Value::Null));
        write_atomic_json(&journal, &json!({"stage":"cleanup", "completed":["service", "models"], "failed":[]}))
            .unwrap();
        write_atomic_json(
            &report,
            &json!({"state":"complete", "completed":["service", "models"], "failed":[], "agents":{}}),
        )
        .unwrap();
        assert_eq!(consume_report_at(&dir).unwrap()["state"], "complete");
        assert_eq!(consume_report_at(&dir), Ok(Value::Null));
        let outside = temp.path().join("outside.json");
        fs::write(&outside, b"{}").unwrap();
        Files::symlink(&outside, &report);
        assert_eq!(consume_report_at(&dir), Err(refused("reset.unsafe")));
        assert!(outside.exists());
    }

    #[test]
    fn records_are_read_within_their_bound_from_a_private_file_and_a_refused_one_stays() {
        let (_temp, dir) = private_dir();
        let (report, journal) = (dir.join(REPORT), dir.join(JOURNAL));
        let oversized = vec![b' '; RECORD_LIMIT as usize + 1];
        Files::write_private(&report, &oversized).unwrap();
        assert_eq!(consume_report_at(&dir), Err(refused("reset.failed")));
        assert!(report.exists());
        fs::remove_file(&report).unwrap();
        Files::write_private(&journal, &oversized).unwrap();
        assert_eq!(finalize_relaunched_reset_at(&dir), Err("report-read"));
        assert!(journal.exists());
        // A journal with a second name is not a private record of ours: refused, and left as it is.
        write_atomic_json(&journal, &json!({"stage":"awaiting-relaunch", "completed":[], "failed":[], "agents":{}}))
            .unwrap();
        fs::hard_link(&journal, dir.join("alias")).unwrap();
        assert_eq!(consume_report_at(&dir), Err(refused("reset.unsafe")));
        assert_eq!(finalize_relaunched_reset_at(&dir), Err("report-read"));
        assert!(journal.exists() && !report.exists());
    }

    #[test]
    fn helper_reports_complete_only_after_relaunched_setup_and_keeps_failed_writes() {
        let (_temp, dir) = private_dir();
        let report = dir.join("report.json");
        let journal = dir.join("journal.json");
        let mut launched = false;
        finish_reset_at(&report, &journal, &["service", "app-data"], vec![], json!({}), write_atomic_json, || {
            assert!(journal.exists());
            assert!(!report.exists());
            launched = true;
            true
        });
        assert!(launched);
        assert!(journal.exists());
        finalize_relaunched_reset_at(&dir).unwrap();
        assert!(!journal.exists());
        assert_eq!(consume_report_at(&dir).unwrap()["state"], "complete");

        let bad_write = |path: &Path, value: &Value| {
            if path == report {
                Err("report-write")
            } else {
                write_atomic_json(path, value)
            }
        };
        finish_reset_at(&report, &journal, &["service"], vec![], json!({}), bad_write, || true);
        assert!(journal.exists());
        assert_eq!(consume_report_at(&dir).unwrap()["state"], "partial");

        finish_reset_at(&report, &journal, &["service"], vec![], json!({}), write_atomic_json, || false);
        let failed = consume_report_at(&dir).unwrap();
        assert_eq!(failed["state"], "partial");
        assert_eq!(failed["failed"][0]["key"], "reset.relaunch-failed");
    }

    #[test]
    fn missing_record_with_residual_connector_data_refuses_absent_retry() {
        let (_temp, home) = fixture();
        let data = standard(&home);
        assert!(!residual_connector_data(&data));
        private_dirs(&home, &data);
        fs::write(data.join("install.lock"), b"marker").unwrap();
        assert!(!residual_connector_data(&data));
        // The engine's models live in the standard `D` on every OS: not the connector's.
        fs::create_dir(data.join(sidevoice_platform::data_dir::MODELS)).unwrap();
        assert!(!residual_connector_data(&data));
        fs::write(data.join("core-secret"), b"sentinel").unwrap();
        assert!(residual_connector_data(&data));
        fs::remove_file(data.join("core-secret")).unwrap();
        fs::create_dir(data.join(".uninstall-recovery")).unwrap();
        assert!(residual_connector_data(&data));
        fs::remove_dir(data.join(".uninstall-recovery")).unwrap();
        fs::create_dir_all(data.join("install")).unwrap();
        assert!(residual_connector_data(&data));
    }
}

/// The reset's connector step against test-support's native fake connector (`fixture::fake_connector`, built fresh from
/// this checkout), on every host.
#[cfg(test)]
mod connector_cli_tests {
    use super::tests::{fixture, private_dirs, standard};
    use super::*;
    use sidevoice_desktop_test_support::fixture as fake;
    use sidevoice_local_host::cli::Cli;

    /// A fake connector at `program` that answers as `config` says.
    fn connector(program: &Path, config: Value) {
        fake::install(&fake::fake_connector(), program, &config).unwrap();
    }

    /// Every run of the fake connector, in order (its append-only `calls` file): empty if it never ran.
    fn calls(log: &Path) -> Vec<String> {
        fs::read_to_string(log).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    #[test]
    fn fake_installed_connectors_refuse_without_safe_capability() {
        let cases = [
            ("missing", json!({"response": {"ok": true, "version": "0.6.0"}}), false),
            ("zero", json!({"response": {"ok": true, "safe_uninstall": 0}}), false),
            ("one", json!({"response": {"ok": true, "safe_uninstall": 1}}), true),
            ("two", json!({"response": {"ok": true, "safe_uninstall": 2}}), true),
            ("garbage", json!({"stdout": "not-json"}), false),
        ];
        for (name, mut config, accepted) in cases {
            let dir = tempfile::tempdir().unwrap();
            let program = dir.path().join(fake::program_name());
            let log = dir.path().join("calls");
            config["calls"] = json!(log);
            connector(&program, config);
            let cli = Cli { prefix: vec![program.to_string_lossy().into_owned()], data: dir.path().join(".sidevoice") };
            let checked = probe_safe_connector(&cli, Duration::from_secs(10));
            assert_eq!(checked.is_ok(), accepted, "{name}");
            assert_eq!(calls(&log), ["--version --json"], "{name}: the probe only asks for the version");
            if checked.is_ok() {
                cli.run(&["uninstall", "--json"], Duration::from_secs(10)).unwrap();
                assert_eq!(calls(&log), ["--version --json", "uninstall --json"], "{name}");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join(fake::program_name());
        connector(&program, json!({"delay_ms": 2000, "response": {"ok": true, "safe_uninstall": 1}}));
        let cli = Cli { prefix: vec![program.to_string_lossy().into_owned()], data: dir.path().join(".sidevoice") };
        assert_eq!(probe_safe_connector(&cli, Duration::from_millis(100)), Err(refused("reset.unsupported-connector")));
    }

    #[test]
    fn retained_connector_record_uses_recovery_executable_on_retry() {
        let (_temp, home) = fixture();
        let data = standard(&home);
        let recovery = data.join(".uninstall-recovery");
        private_dirs(&home, &recovery);
        let program = recovery.join(fake::program_name());
        let log = home.join("retry-calls");
        connector(&program, json!({"calls": log, "response": {"ok": true, "safe_uninstall": 1, "state": "removed"}}));
        let releases = data.join("install");
        fs::write(data.join("install.json"), json!({"releases": releases.clone(), "command": [program]}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data, &data), Ok(releases));
        let Ok(cli) = crate::local_host::connector_cli(&data) else { panic!("the recorded recovery executable runs") };
        assert_eq!(probe_safe_connector(&cli, Duration::from_secs(10)), Ok(()));
        assert_eq!(calls(&log), ["--version --json"], "the probe only asks for the version");
        assert_eq!(cli.run(&["uninstall", "--json"], Duration::from_secs(10)).unwrap()["state"], "removed");
        assert_eq!(calls(&log), ["--version --json", "uninstall --json"]);
    }
}

/// The uninstall's decisions, on every host (only macOS removes the app: `uninstall::available`).
#[cfg(test)]
mod uninstall_tests {
    use super::*;

    fn plain_write(path: &Path, value: &Value) -> Result<(), &'static str> {
        fs::write(path, serde_json::to_vec(value).unwrap()).map_err(|_| "report-write")
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn a_clean_uninstall_trashes_the_app_without_relaunch_or_report() {
        let dir = tempfile::tempdir().unwrap();
        let (report, journal) = (dir.path().join("report.json"), dir.path().join("journal.json"));
        let mut trashed = false;
        let agents = json!({"codex": "foreign"});
        let failed = vec![json!({"step": "agents", "key": "reset.agent-remains", "agent": "codex"})];
        finish_uninstall_at(
            &report,
            &journal,
            &["service", "app-data"],
            failed,
            agents,
            |path, value| {
                assert_eq!(value["stage"], "app");
                plain_write(path, value)
            },
            || {
                assert!(journal.exists(), "the journal names the step in progress");
                trashed = true;
                Ok(())
            },
            || panic!("an uninstalled app is not relaunched"),
        );
        assert!(trashed);
        assert!(!journal.exists() && !report.exists());
    }

    #[test]
    fn a_failed_data_step_keeps_the_app_and_relaunches_to_report() {
        let dir = tempfile::tempdir().unwrap();
        let (report, journal) = (dir.path().join("report.json"), dir.path().join("journal.json"));
        let mut relaunched = false;
        finish_uninstall_at(
            &report,
            &journal,
            &["service"],
            vec![json!({"step": "webview", "key": "reset.process-busy"})],
            json!({}),
            plain_write,
            || panic!("an app whose data remains is not trashed"),
            || {
                relaunched = true;
                true
            },
        );
        assert!(relaunched);
        let pending = read(&journal);
        assert_eq!(pending["stage"], "awaiting-relaunch");
        assert_eq!(pending["failed"][1], json!({"step": "app", "key": "reset.app-kept"}));
    }

    #[test]
    fn a_failed_move_relaunches_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let (report, journal) = (dir.path().join("report.json"), dir.path().join("journal.json"));
        finish_uninstall_at(
            &report,
            &journal,
            &["service"],
            vec![],
            json!({}),
            plain_write,
            || Err("reset.app-trash-failed"),
            || true,
        );
        let pending = read(&journal);
        assert_eq!(pending["stage"], "awaiting-relaunch");
        assert_eq!(pending["failed"], json!([{"step": "app", "key": "reset.app-trash-failed"}]));

        // Neither moved nor relaunched: the next launch reads a partial report.
        fs::remove_file(&journal).unwrap();
        finish_uninstall_at(
            &report,
            &journal,
            &["service"],
            vec![],
            json!({}),
            plain_write,
            || Err("reset.app-trash-failed"),
            || false,
        );
        let value = read(&report);
        assert_eq!(value["state"], "partial");
        assert_eq!(value["failed"][0]["key"], "reset.app-trash-failed");
        assert_eq!(value["failed"][1]["key"], "reset.relaunch-failed");
    }
}
