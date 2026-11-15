//! Crash reports on the phone (sidevoice-shared `crash`): the reporter and what feeds it, under the person's setting
//! (Settings → Crash reports, `host.crashReports`), which the app keeps in its own files and holds to before any page has
//! loaded. The DSN is baked in by a release or nightly build (`SIDEVOICE_SENTRY_DSN`, set by CI); a local or dev build has
//! none and sends nothing, whatever the setting says.
//!
//! What is sent from a phone: the app's own panics, from the panic hook (`panics`), which tells the reporter of each. The
//! crash records of the machines the phone is paired with are not read here yet: the phone reaches a machine through the
//! room page's transport (relay included), which the app's Rust does not speak. A native crash (a signal, an abort that
//! leaves no panic) is not captured either: that needs the platform SDKs and a decision about memory capture.

use sidevoice_shared::crash::{self, Config, Reporter};
use sidevoice_shared_tauri::CrashReports;
use tauri::{AppHandle, Manager};

/// The DSN of a release or nightly build; empty or absent otherwise.
const DSN: Option<&str> = option_env!("SIDEVOICE_SENTRY_DSN");

/// At start, right after the log: the reporter (the setting is read here) and the observer the panic hook tells.
pub fn setup(app: &AppHandle) {
    let Ok(dir) = crate::own_files(app) else {
        return;
    };
    let reporter = Reporter::start(Config {
        resource: crate::platform::resource::current(),
        dsn: DSN.map(str::to_owned),
        dir,
        post: None,
    });
    crash::report_panics(reporter.clone());
    app.manage(CrashReports(reporter));
}
