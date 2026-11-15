//! Crash reports on the phone (sidevoice-shared `crash`): the reporter and what feeds it, under the person's setting
//! (Settings → Crash reports, `host.crashReports`), which the app keeps in its own files and holds to before any page has
//! loaded. The DSN is baked in by a release or nightly build (`SIDEVOICE_SENTRY_DSN`, set by CI); a local or dev build has
//! none and sends nothing, whatever the setting says.
//!
//! What is sent from a phone: the app's own panics, from the panic hook (the shared logging's panic hook), which tells the reporter of each. The
//! crash records of the machines the phone is paired with are not read here yet: the phone reaches a machine through the
//! room page's transport (relay included), which the app's Rust does not speak. A native crash (a signal, an abort that
//! leaves no panic) is not captured either: that needs the platform SDKs and a decision about memory capture.

use sidevoice_shared::crash::{self, Config, Reporter};
use sidevoice_shared_tauri::CrashReports;
use tauri::{AppHandle, Manager};

/// The DSN of a release or nightly build; empty or absent otherwise.
const DSN: Option<&str> = option_env!("SIDEVOICE_SENTRY_DSN");

/// At start, right after the log: the reporter (the setting is read here; the client is made on a thread of its own) and the
/// observer the panic hook tells. With no files folder, or a reporter that cannot start, reporting is off for this run and
/// the setting still answers the page.
pub fn setup(app: &AppHandle) {
    let resource = crate::platform::resource::current();
    let Ok(dir) = crate::own_files(app) else {
        app.manage(CrashReports(Reporter::unavailable(resource)));
        return;
    };
    let reporter = Reporter::start_in_background(Config {
        resource,
        dsn: DSN.map(str::to_owned),
        dir,
        post: None,
    });
    crash::report_panics(reporter.clone());
    app.manage(CrashReports(reporter));
}
