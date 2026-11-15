//! Crash reports on the desktop (sidevoice-shared `crash`): the reporter, the panic hook that feeds it, and the pass that
//! sends this computer's own core's crash records.
//!
//! The setting is the person's (Settings → Crash reports, `host.crashReports`), kept in the app's data folder and held to
//! before any page has loaded. The DSN is baked in by a release or nightly build (`SIDEVOICE_SENTRY_DSN`, set by CI); a
//! local or dev build has none and sends nothing, whatever the setting says.
//!
//! The records of this computer's core are read with the local device's token and acknowledged only after Sentry took
//! them (`machine::sync`). Under "never" the pass does not call the core at all. The machines the room page reaches
//! over the network are not read here yet.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sidevoice_local_host::host::LocalHost;
use sidevoice_shared::crash::{self, Config, Machine, MachineError, Reporter};
use sidevoice_shared_tauri::CrashReports;
use tauri::{AppHandle, Manager};

/// The DSN of a release or nightly build; empty or absent otherwise.
const DSN: Option<&str> = option_env!("SIDEVOICE_SENTRY_DSN");

/// How often the core's records are looked for, once the first pass (a little after launch) has run.
const EVERY: Duration = Duration::from_secs(5 * 60);
const FIRST: Duration = Duration::from_secs(45);

/// This computer's core, as a machine whose records the app sends.
struct LocalCore(Arc<LocalHost>);

impl Machine for LocalCore {
    fn list(&self) -> Result<Vec<Value>, MachineError> {
        self.0.crash_records().map_err(|_| MachineError::Unavailable)
    }

    fn acknowledge(&self, id: &str) -> Result<(), MachineError> {
        self.0.acknowledge_crash(id).map_err(|_| MachineError::Unavailable)
    }
}

/// At start, right after the log: the reporter (the setting is read here; the client is made on a thread of its own), the
/// panic hook's observer, and the pass over the core's records on a thread of its own. Nothing here can stop the app: with no
/// data folder, or a reporter that cannot start, reporting is off for this run and the setting still answers the page.
pub fn setup(app: &AppHandle) {
    let resource = crate::platform::resource::current();
    let Ok(dir) = crate::data::desktop_dir(app) else {
        app.manage(CrashReports(Reporter::unavailable(resource)));
        return;
    };
    let reporter = Reporter::start_in_background(Config { resource, dsn: DSN.map(str::to_owned), dir, post: None });
    crash::report_panics(reporter.clone());
    app.manage(CrashReports(reporter.clone()));
    let app = app.clone();
    let started = std::thread::Builder::new().name("crash-records".into()).spawn(move || {
        // A pass that panics is contained and the next one comes after a longer wait: the loop does not end.
        crash::machine::poll_forever(&reporter, FIRST, EVERY, |reporter| {
            if let Some(host) = crate::local_host::running(&app) {
                crash::machine::sync(reporter, &LocalCore(host));
            }
        });
    });
    if let Err(error) = started {
        tracing::warn!(
            error.type = crate::error::DesktopError::CrashRecordsNotStarted.code(),
            kind = ?error.kind(),
            "the pass over the core's crash records could not start"
        );
    }
}
