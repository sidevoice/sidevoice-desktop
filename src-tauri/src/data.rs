//! The app's private pairing file, the machine's shared model root, and the confirmed reset.
//! Only the current room page can ask; no bridge method accepts a filesystem path.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Manager, Webview};

use crate::platform::host::RESET_AVAILABLE;
use crate::platform::reset_handoff::{self, Handoff};
use crate::platform::{account, app_data, credentials, uninstall};
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

#[cfg(target_os = "macos")]
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

fn pairings_file(app: &AppHandle) -> Result<PathBuf, Value> {
    Ok(desktop_dir(app)?.join("pairings.json"))
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
    if !RESET_AVAILABLE {
        return Err(refused("reset.unavailable"));
    }
    let path = report_path().ok_or_else(|| refused("reset.unavailable"))?;
    consume_report_at(&path)
}

fn consume_report_at(path: &Path) -> Answer {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refused("reset.unsafe"));
    }
    match fs::read(path) {
        Ok(bytes) if bytes.len() <= 16 * 1024 => {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.failed"))?;
            let journal = path.with_file_name("journal.json");
            if let Err(error) = fs::remove_file(&journal) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(refused("reset.report-failed"));
                }
            }
            fs::remove_file(path).map_err(|_| refused("reset.failed"))?;
            Ok(value)
        }
        Ok(_) => Err(refused("reset.failed")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let journal = path.with_file_name("journal.json");
            if fs::symlink_metadata(&journal).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(refused("reset.unsafe"));
            }
            match fs::read(&journal) {
                Ok(bytes) if bytes.len() <= 16 * 1024 => {
                    let progress: Value = serde_json::from_slice(&bytes).map_err(|_| refused("reset.report-failed"))?;
                    let mut failed = progress.get("failed").and_then(Value::as_array).cloned().unwrap_or_default();
                    failed.push(json!({"step":"helper", "key":"reset.interrupted"}));
                    fs::remove_file(&journal).map_err(|_| refused("reset.report-failed"))?;
                    Ok(
                        json!({"state":"partial", "completed":progress.get("completed").cloned().unwrap_or_else(|| json!([])), "failed":failed, "agents":{}}),
                    )
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
                _ => Err(refused("reset.report-failed")),
            }
        }
        Err(_) => Err(refused("reset.failed")),
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

/// A host without the reset transaction refuses before the confirmation and before any data is touched.
async fn confirmed_reset(app: AppHandle, uninstall: bool) -> Answer {
    use crate::platform::confirmation::{self, ConfirmationKind};
    use crate::platform::webview_data;
    if !RESET_AVAILABLE {
        return Err(refused("reset.unavailable"));
    }
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
/// in progress. Where there is no reset transaction (`RESET_AVAILABLE`), `confirmed_reset` refuses before this; this
/// refuses again.
fn refuse_before_confirmation(app: &AppHandle) -> Result<(), &'static str> {
    if !RESET_AVAILABLE {
        return Err("reset.unavailable");
    }
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
    let (mut failed, agents, service_outcome) = reset_before_exit()?;
    // The provider keys, on every host that runs the transaction, journalled as the last step before exit.
    if credentials::clear_all().is_err() {
        failed.push(json!({"step": "keychain", "key": "reset.keychain-failed"}));
    }
    let journal = report_path().ok_or_else(|| refused("reset.helper-failed"))?.with_file_name("journal.json");
    write_atomic_json(
        &journal,
        &json!({"stage":"keychain", "completed":["service", "keychain"], "failed":failed.clone()}),
    )
    .map_err(|_| refused("reset.report-failed"))?;
    let pending = reset_handoff::launch(
        admission,
        ResetPlan { parent_pid: std::process::id(), uninstall, initial: json!({"failed": failed, "agents": agents}) },
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

/// The existing macOS steps before the app exits, under the held admission: the connector service and its
/// agents, journalled; `begin_reset` then clears the provider keys. Returns the failures so far, the agents' results
/// and the service outcome.
#[cfg(target_os = "macos")]
fn reset_before_exit() -> Result<(Vec<Value>, Value, &'static str), Value> {
    use sidevoice_local_host::{cli::Cli, paths::DataDirs};
    let dirs = DataDirs::from_env().ok_or_else(|| refused("reset.unavailable"))?;
    // `HOME` only when it is this account's home in the account database: no other home is reset.
    let home = account::home().map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    validate_uninstall_scope_at(&home, &dirs.data)
        .map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    let installed_cli = match Cli::installed(&dirs) {
        Ok(cli) => {
            probe_safe_connector(&cli, Duration::from_secs(10))?;
            Some(cli)
        }
        Err(_) if !dirs.install_record().exists() => {
            // A missing record is only an absent service when all owned data is gone too.
            if !fresh_service_absent(&home) || residual_connector_data(&dirs.data) {
                return Err(refused("reset.service-failed"));
            }
            None
        }
        Err(_) => return Err(refused("reset.unsafe")),
    };
    let report = report_path().ok_or_else(|| refused("reset.helper-failed"))?;
    if let Err(error) = fs::remove_file(&report) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(refused("reset.report-failed"));
        }
    }
    let journal = report.with_file_name("journal.json");
    write_atomic_json(&journal, &json!({"stage":"preflight", "completed":[], "failed":[]}))
        .map_err(|_| refused("reset.helper-failed"))?;
    let (service, service_outcome) = match installed_cli {
        Some(cli) => (
            run_checked_uninstall(&home, &dirs.data, || {
                cli.run(&["uninstall", "--json"], Duration::from_secs(180)).map_err(|_| refused("reset.service-failed"))
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

/// The reset transaction exists only on macOS; `confirmed_reset` refuses before this elsewhere.
#[cfg(not(target_os = "macos"))]
fn reset_before_exit() -> Result<(Vec<Value>, Value, &'static str), Value> {
    Err(refused("reset.unavailable"))
}

#[cfg(target_os = "macos")]
fn require_safe_uninstall(version: &Value) -> Result<(), Value> {
    if version.get("ok") == Some(&Value::Bool(true))
        && version.get("safe_uninstall").and_then(Value::as_u64).is_some_and(|level| level >= 1)
    {
        Ok(())
    } else {
        Err(refused("reset.unsupported-connector"))
    }
}

#[cfg(target_os = "macos")]
fn probe_safe_connector(cli: &sidevoice_local_host::cli::Cli, timeout: Duration) -> Result<(), Value> {
    let version = cli.run(&["--version", "--json"], timeout).map_err(|_| refused("reset.unsupported-connector"))?;
    require_safe_uninstall(&version)
}

#[cfg(target_os = "macos")]
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
        // `Models` is the engine's (docs/ENGINES.md); on macOS its place is `D`: not the connector's to leave behind.
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
    let bytes = serde_json::to_vec(value).map_err(|_| "report-write")?;
    if bytes.len() > 16 * 1024 {
        return Err("report-write");
    }
    Files::write_private(path, &bytes).map_err(|_| "report-write")
}

#[cfg(target_os = "macos")]
fn fresh_service_absent(home: &Path) -> bool {
    fresh_service_absent_with(
        home,
        |label| {
            let target = format!("gui/{}/{label}", unsafe { libc::geteuid() });
            let Ok(output) = Command::new("/bin/launchctl").args(["print", &target]).output() else { return false };
            if output.status.success() {
                return false;
            }
            let detail = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            output.status.code() == Some(113) || detail.contains("could not find service")
        },
        || {
            let Ok(output) = Command::new("/bin/ps").args(["-axo", "comm="]).output() else { return false };
            output.status.success() && !managed_process_in_ps(&output.stdout)
        },
    )
}

#[cfg(target_os = "macos")]
fn managed_process_in_ps(output: &[u8]) -> bool {
    String::from_utf8_lossy(output).lines().any(|line| {
        let path = Path::new(line.trim());
        matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("sidevoice-core-rust" | "sidevoice-core" | "sidevoice-connector")
        )
    })
}

#[cfg(target_os = "macos")]
fn fresh_service_absent_with(
    home: &Path,
    launchd_absent: impl Fn(&str) -> bool,
    processes_absent: impl Fn() -> bool,
) -> bool {
    if fs::canonicalize(home).ok().as_deref() != Some(home) {
        return false;
    }
    for label in ["dev.sidevoice.core", "dev.sidevoice.connector"] {
        match fs::symlink_metadata(home.join("Library/LaunchAgents").join(format!("{label}.plist"))) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return false,
        }
        if !launchd_absent(label) {
            return false;
        }
    }
    processes_absent()
}

#[cfg(target_os = "macos")]
fn run_checked_uninstall<T>(home: &Path, data: &Path, invoke: impl FnOnce() -> Result<T, Value>) -> Result<T, Value> {
    // Still this account's home in the account database, right before the destructive call.
    if account::home().as_deref() != Ok(home) {
        return Err(json!({"key": "reset.unsafe", "reason": "unsafe-home"}));
    }
    validate_uninstall_scope_at(home, data).map_err(|reason| json!({"key": "reset.unsafe", "reason": reason}))?;
    invoke()
}

/// `D` on macOS with no override: the OS's standard place of `sidevoice-platform`'s `data_dir`.
#[cfg(target_os = "macos")]
fn standard_data(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Sidevoice")
}

#[cfg(target_os = "macos")]
fn validate_uninstall_scope_at(home: &Path, data: &Path) -> Result<PathBuf, &'static str> {
    if !clean_absolute(home) || fs::canonicalize(home).ok().as_deref() != Some(home) {
        return Err("unsafe-home");
    }
    // `D` on macOS: the standard place of `sidevoice-platform`'s `data_dir`, and the releases `D/install` in it. An
    // override (`SIDEVOICE_DATA_DIR`) is not a known directory.
    let expected_data = standard_data(home);
    if data != expected_data || !clean_absolute(data) {
        return Err("outside-known-dirs");
    }
    let expected_releases = sidevoice_local_host::paths::DataDirs::new(&expected_data).releases();
    let record = data.join("install.json");
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
        let recovery = expected_data.join(".uninstall-recovery/sidevoice-connector");
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

/// `R/releases/<id>/bin/sidevoice-connector`: the command the connector's `install` records for the release it
/// selects, `<id>` one plain name.
#[cfg(target_os = "macos")]
fn is_release_binary(path: &Path, releases: &Path) -> bool {
    use std::path::Component;
    let Ok(rest) = path.strip_prefix(releases.join("releases")) else { return false };
    let parts: Vec<_> = rest.components().collect();
    matches!(parts.as_slice(), [Component::Normal(id), Component::Normal(bin), Component::Normal(name)]
        if !id.to_string_lossy().starts_with('.') && *bin == "bin" && *name == "sidevoice-connector")
}

/// The report sits in the handoff's private control directory, outside every reset deletion root.
fn report_path() -> Option<PathBuf> {
    use sidevoice_platform::fs::{Files, Fs};
    let dir = reset_handoff::state_dir().ok()?;
    Files::ensure_private_dir(&dir).ok()?;
    Some(dir.join("report.json"))
}

#[cfg(target_os = "macos")]
fn clean_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|part| matches!(part, std::path::Component::RootDir | std::path::Component::Normal(_)))
}

/// Called in the helper process, before Tauri. The authenticated plan arrives on every host.
pub fn run_reset_helper() {
    let Ok(helper) = Handoff::new().and_then(|handoff| handoff.receive()) else { return };
    let (parent_pid, uninstall, initial) = (helper.plan.parent_pid, helper.plan.uninstall, helper.plan.initial.clone());
    if !RESET_AVAILABLE {
        // Complete the committed wait and release it; nothing is cleaned up.
        drop(helper.await_exit());
    } else {
        clean_up_after_exit(helper, parent_pid, uninstall, initial);
    }
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
    let Some(report) = report_path() else { return };
    let Some(initial_failed) = initial.get("failed").and_then(Value::as_array) else { return };
    let mut completed = Vec::new();
    let mut failed = initial_failed.clone();
    let journal = report.with_file_name("journal.json");
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
        std::env::current_exe()
            .ok()
            .and_then(|exe| Command::new(exe).env("SIDEVOICE_RESET_RELAUNCH", "1").spawn().ok())
            .is_some()
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

#[cfg(target_os = "macos")]
pub fn finalize_relaunched_reset() {
    if std::env::var("SIDEVOICE_RESET_RELAUNCH").as_deref() != Ok("1") {
        return;
    }
    let Some(report) = report_path() else { return };
    let journal = report.with_file_name("journal.json");
    let _ = finalize_relaunched_reset_at(&report, &journal);
}

#[cfg(target_os = "macos")]
fn finalize_relaunched_reset_at(report: &Path, journal: &Path) -> Result<(), &'static str> {
    let bytes = fs::read(journal).map_err(|_| "report-read")?;
    if bytes.len() > 16 * 1024 {
        return Err("report-read");
    }
    let pending: Value = serde_json::from_slice(&bytes).map_err(|_| "report-read")?;
    if pending["stage"] != "awaiting-relaunch" {
        return Err("report-stage");
    }
    let failed = pending.get("failed").and_then(Value::as_array).ok_or("report-stage")?;
    let value = json!({"state":if failed.is_empty() {"complete"} else {"partial"},
        "completed":pending["completed"],"failed":failed,"agents":pending["agents"]});
    write_atomic_json(report, &value)?;
    fs::remove_file(journal).map_err(|_| "report-remove")?;
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn connector_uninstall_scope_refuses_overrides_before_fake_cli_runs() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let mut invoked = false;
        for data in
            [home.clone(), PathBuf::from("/"), home.join("../other"), home.join("Documents"), PathBuf::from("relative")]
        {
            assert!(run_checked_uninstall(&home, &data, || {
                invoked = true;
                Ok(())
            })
            .is_err());
        }
        assert!(!invoked);
        let data = home.join("Library/Application Support/Sidevoice");
        assert_eq!(
            validate_uninstall_scope_at(&home, &home.join(".sidevoice")),
            Err("outside-known-dirs"),
            "the old directory is not known"
        );
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.json"), json!({"releases": home.join("Documents/sidevoice")}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Err("outside-known-dirs"));
        let releases = data.join("install");
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [releases.join("releases/1.2.3/bin/sidevoice-connector")]})
                .to_string(),
        )
        .unwrap();
        assert!(validate_uninstall_scope_at(&home, &data).is_ok());
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [data.join(".uninstall-recovery/sidevoice-connector")]})
                .to_string(),
        )
        .unwrap();
        assert!(validate_uninstall_scope_at(&home, &data).is_ok());
        fs::write(data.join("install.json"), json!({"releases": releases, "command": ["/usr/bin/rm"]}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Err("unsafe-record"));
        // Not the layout the connector records: `current` is a link of the old installer, not a release.
        fs::write(
            data.join("install.json"),
            json!({"releases": releases, "command": [releases.join("current/bin/sidevoice-connector")]}).to_string(),
        )
        .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Err("unsafe-record"));
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
    fn fake_installed_connectors_refuse_without_safe_capability() {
        use sidevoice_local_host::cli::Cli;
        use std::os::unix::fs::PermissionsExt;
        let cases = [
            ("missing", r#"{"ok":true,"version":"0.6.0"}"#, false),
            ("zero", r#"{"ok":true,"safe_uninstall":0}"#, false),
            ("one", r#"{"ok":true,"safe_uninstall":1}"#, true),
            ("two", r#"{"ok":true,"safe_uninstall":2}"#, true),
            ("garbage", "not-json", false),
        ];
        for (name, answer, accepted) in cases {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("sidevoice-connector");
            let marker = dir.path().join("uninstall-invoked");
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = \"--version\" ] && [ \"$2\" = \"--json\" ]; then\n  printf '%s\\n' '{answer}'\nelse\n  : > '{}'\n  printf '%s\\n' '{{\"ok\":true}}'\nfi\n",
                    marker.display()
                ),
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
            let cli = Cli { prefix: vec![script.to_string_lossy().into_owned()], data: dir.path().join(".sidevoice") };
            let checked = probe_safe_connector(&cli, Duration::from_secs(2));
            assert_eq!(checked.is_ok(), accepted, "{name}");
            if checked.is_ok() {
                cli.run(&["uninstall", "--json"], Duration::from_secs(2)).unwrap();
            }
            assert_eq!(marker.exists(), accepted, "{name}: unsafe uninstall invocation");
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("slow-sidevoice-connector");
        fs::write(&script, "#!/bin/sh\nsleep 2\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let cli = Cli { prefix: vec![script.to_string_lossy().into_owned()], data: dir.path().join(".sidevoice") };
        assert_eq!(probe_safe_connector(&cli, Duration::from_millis(100)), Err(refused("reset.unsupported-connector")));
    }

    #[test]
    fn retained_connector_record_uses_recovery_executable_on_retry() {
        use sidevoice_local_host::{cli::Cli, paths::DataDirs};
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let data = home.join("Library/Application Support/Sidevoice");
        let recovery = data.join(".uninstall-recovery");
        fs::create_dir_all(&recovery).unwrap();
        let script = recovery.join("sidevoice-connector");
        let marker = home.join("retry-invoked");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' '{{\"ok\":true,\"safe_uninstall\":1}}'\nelse\n  : > '{}'\n  printf '%s\\n' '{{\"ok\":true,\"state\":\"removed\"}}'\nfi\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let releases = data.join("install");
        fs::write(data.join("install.json"), json!({"releases": releases.clone(), "command": [script]}).to_string())
            .unwrap();
        assert_eq!(validate_uninstall_scope_at(&home, &data), Ok(releases));
        let cli = Cli::installed(&DataDirs::new(&data)).unwrap();
        assert_eq!(probe_safe_connector(&cli, Duration::from_secs(2)), Ok(()));
        assert!(!marker.exists());
        assert_eq!(cli.run(&["uninstall", "--json"], Duration::from_secs(2)).unwrap()["state"], "removed");
        assert!(marker.exists());
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

    /// The report's directory as `report_path()` makes it: private, as the shared record writer requires.
    fn private_dir() -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700)).tempdir().unwrap()
    }

    #[test]
    fn durable_journal_reports_interruption_then_allows_retry() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_dir();
        let report = dir.path().join("report.json");
        let journal = dir.path().join("journal.json");
        write_atomic_json(&journal, &json!({"stage":"service", "completed":["service"], "failed":[]})).unwrap();
        assert_eq!(fs::metadata(&journal).unwrap().permissions().mode() & 0o077, 0);
        let interrupted = consume_report_at(&report).unwrap();
        assert_eq!(interrupted["state"], "partial");
        assert_eq!(interrupted["completed"], json!(["service"]));
        assert_eq!(interrupted["failed"][0]["key"], "reset.interrupted");
        assert_eq!(consume_report_at(&report), Ok(Value::Null));
        write_atomic_json(&journal, &json!({"stage":"cleanup", "completed":["service", "models"], "failed":[]}))
            .unwrap();
        write_atomic_json(
            &report,
            &json!({"state":"complete", "completed":["service", "models"], "failed":[], "agents":{}}),
        )
        .unwrap();
        assert_eq!(consume_report_at(&report).unwrap()["state"], "complete");
        assert_eq!(consume_report_at(&report), Ok(Value::Null));
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), &report).unwrap();
        assert_eq!(consume_report_at(&report), Err(refused("reset.unsafe")));
        assert!(outside.path().exists());
    }

    #[test]
    fn helper_reports_complete_only_after_relaunched_setup_and_keeps_failed_writes() {
        let dir = private_dir();
        let report = dir.path().join("report.json");
        let journal = dir.path().join("journal.json");
        let mut launched = false;
        finish_reset_at(&report, &journal, &["service", "app-data"], vec![], json!({}), write_atomic_json, || {
            assert!(journal.exists());
            assert!(!report.exists());
            launched = true;
            true
        });
        assert!(launched);
        assert!(journal.exists());
        finalize_relaunched_reset_at(&report, &journal).unwrap();
        assert!(!journal.exists());
        assert_eq!(consume_report_at(&report).unwrap()["state"], "complete");

        let bad_write = |path: &Path, value: &Value| {
            if path == report {
                Err("report-write")
            } else {
                write_atomic_json(path, value)
            }
        };
        finish_reset_at(&report, &journal, &["service"], vec![], json!({}), bad_write, || true);
        assert!(journal.exists());
        assert_eq!(consume_report_at(&report).unwrap()["state"], "partial");

        finish_reset_at(&report, &journal, &["service"], vec![], json!({}), write_atomic_json, || false);
        let failed = consume_report_at(&report).unwrap();
        assert_eq!(failed["state"], "partial");
        assert_eq!(failed["failed"][0]["key"], "reset.relaunch-failed");
    }

    #[test]
    fn missing_install_record_requires_fresh_launchd_and_process_absence() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        assert!(!fresh_service_absent_with(&home, |_| false, || true));
        assert!(!fresh_service_absent_with(&home, |_| true, || false));
        assert!(fresh_service_absent_with(&home, |_| true, || true));
        fs::create_dir_all(home.join("Library/LaunchAgents")).unwrap();
        fs::write(home.join("Library/LaunchAgents/dev.sidevoice.core.plist"), b"orphan").unwrap();
        assert!(!fresh_service_absent_with(&home, |_| true, || true));
    }

    #[test]
    fn production_process_parser_recognizes_bundled_core() {
        assert!(managed_process_in_ps(
            b"/Users/test/Library/Application Support/Sidevoice/install/releases/1.2.3/core/bin/sidevoice-core-rust\n"
        ));
        assert!(managed_process_in_ps(
            b"/Users/test/Library/Application Support/Sidevoice/install/releases/1.2.3/bin/sidevoice-connector\n"
        ));
        assert!(!managed_process_in_ps(b"/usr/bin/other-service\n"));
    }

    #[test]
    fn missing_record_with_residual_connector_data_refuses_absent_retry() {
        let home = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(home.path()).unwrap();
        let data = home.join("Library/Application Support/Sidevoice");
        assert!(!residual_connector_data(&data));
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("install.lock"), b"marker").unwrap();
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

/// The uninstall's platform-neutral decisions run on every platform's tests; the macOS-only parts above do not.
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
