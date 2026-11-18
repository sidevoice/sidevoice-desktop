//! Crash reports on the phone (sidevoice-shared `crash`): the reporter and what feeds it, under the person's setting
//! (Settings → Crash reports, `host.crashReports`), which the app keeps in its own files and holds to before any page has
//! loaded. The DSN is baked in by a release or nightly build (`SIDEVOICE_SENTRY_DSN`, set by CI); a local or dev build has
//! none and records and sends nothing, whatever the setting says.
//!
//! What the phone reports: its own panics. The shared logging's panic hook tells the reporter of each, which leaves a record in
//! the app's files; the next start sends it. The phone reads nothing of a machine's (each piece sends its own). A native crash (a
//! signal, an abort that leaves no panic) is not captured: that needs the platform SDKs and a decision about memory capture.
//! The guideline's mobile exception (a hook that cannot log re-emits the record at the next launch) is not made here: the
//! shared hook logs inside the hook, as on the desktop.
//!
//! The phone also reports its **handled errors** under the same switch (sidevoice-shared `crash::handled`, the same code as the
//! desktop's): the `error` events of its own log targets whose `error.type` is one of the codes of its own error enums, one a day
//! per code, with only that code and the place that logged it. The phone has no error enum of its own yet, so for now these are the
//! shared crate's (`SharedError`); an enum added here joins the list below.

use sidevoice_shared::crash::{self, Config, Reporter};
use sidevoice_shared::error::SharedError;
use sidevoice_shared_tauri::CrashReports;
use tauri::{AppHandle, Manager};

/// The DSN of a release or nightly build; empty or absent otherwise.
const DSN: Option<&str> = option_env!("SIDEVOICE_SENTRY_DSN");

/// At start, right after the log: the process's reporter (the setting is read here; the records a previous run left are sent on a
/// thread of its own), with the observer the panic hook tells and the handled errors' sender bound to it. With no files folder, or a
/// reporter that cannot start, reporting is off for this run and the setting still answers the page.
///
/// **Once per process.** `run_app` runs again when Android recreates the activity, so this does too: the hooks are the process's and
/// bind to the first reporter only, and a second reporter would hold its own copy of the setting (a "No enviar" chosen on it would
/// leave the first sending, and the first would write `auto` back over the choice). `crash::start_process_reporter` answers every
/// later call with the first reporter, which is the one each new app state manages.
pub fn setup(app: &AppHandle) {
    let resource = crate::platform::resource::current();
    let Ok(dir) = crate::own_files(app) else {
        app.manage(CrashReports(
            crash::process_reporter().unwrap_or_else(|| Reporter::unavailable(resource)),
        ));
        return;
    };
    // The phone's own codes: the closed list a handled error is reported under.
    let reporter = crash::start_process_reporter(
        Config {
            resource,
            dsn: DSN.map(str::to_owned),
            dir,
            post: None,
        },
        SharedError::codes(),
    );
    app.manage(CrashReports(reporter));
}
