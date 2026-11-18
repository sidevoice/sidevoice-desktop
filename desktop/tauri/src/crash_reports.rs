//! Crash reports on the desktop (sidevoice-shared `crash`): the reporter, the panic hook that feeds it, and the copy of the app's
//! switch to the core this app installed on this computer.
//!
//! The setting is the person's (Settings → Crash reports, `host.crashReports`), kept in the app's data folder and held to before
//! any page has loaded. The DSN is baked in by a release or nightly build (`SIDEVOICE_SENTRY_DSN`, set by CI); a local or dev
//! build has none and records and sends nothing, whatever the setting says. The app reports its own panics only: it reads,
//! relays and acknowledges nothing of the core's or the connector's (each piece sends its own).
//!
//! The switch goes one way, from the app to the core it installed (the logging guideline, section 10): once when the app's own
//! install puts a core on this computer ([`core_installed`]) and each time the person changes the app's switch, through the
//! core's local-only endpoint on its socket. Never on a start, never when an update replaces the binaries, never read back. A
//! core the person installed by hand is not touched; its switch is its own.

use sidevoice_shared::crash::{self, Config, Mode, Reporter};
use sidevoice_shared::logging;
use sidevoice_shared_tauri::CrashReports;
use tauri::{AppHandle, Manager};

/// The DSN of a release or nightly build; empty or absent otherwise.
const DSN: Option<&str> = option_env!("SIDEVOICE_SENTRY_DSN");

/// At start, right after the log: the reporter (the setting is read here; the records a previous run left are sent on a thread
/// of its own) and the panic hook's observer. Nothing here can stop the app: with no data folder, or a reporter that cannot
/// start, reporting is off for this run and the setting still answers the page.
pub fn setup(app: &AppHandle) {
    let resource = crate::platform::resource::current();
    let Ok(dir) = crate::data::desktop_dir(app) else {
        app.manage(CrashReports(Reporter::unavailable(resource)));
        return;
    };
    let reporter = Reporter::start(Config { resource, dsn: DSN.map(str::to_owned), dir, post: None });
    crash::report_panics(reporter.clone());
    let handle = app.clone();
    reporter.on_local_core_change(move |mode| copy_to_core(&handle, mode));
    app.manage(CrashReports(reporter));
}

/// The app's own install has just put a core on this computer (its first: `LocalHost::on_installed`): from now on the person's
/// changes of the switch follow to it, and the switch as it is now goes there once.
pub fn core_installed(app: &AppHandle) {
    let Some(reports) = app.try_state::<CrashReports>() else { return };
    if let Some(mode) = reports.0.local_core_installed() {
        copy_to_core(app, mode);
    }
}

/// Copies the app's switch (`auto` is "send") to the core on this computer, on a thread of its own: the person's click and the
/// install's answer never wait for a core. One try; a core that cannot be reached is a warning and nothing more: the core keeps
/// its own switch until the person changes the app's again.
fn copy_to_core(app: &AppHandle, mode: Mode) {
    let app = app.clone();
    let started = std::thread::Builder::new().name("crash-switch".into()).spawn(move || {
        let outcome = match crate::local_host::running(&app) {
            Some(host) => host.set_crash_reports(mode == Mode::Auto).map_err(|refusal| refusal.key),
            None => Err("local-host.off".to_owned()),
        };
        if let Err(reason) = outcome {
            tracing::warn!(
                error.type = crate::error::DesktopError::CrashSwitchNotCopied.code(),
                reason = logging::checked_code(&reason),
                "the crash report switch could not be copied to the core on this computer; the core keeps its own"
            );
        }
    });
    if let Err(error) = started {
        tracing::warn!(
            error.type = crate::error::DesktopError::CrashSwitchNotCopied.code(),
            kind = ?error.kind(),
            "the crash report switch could not be copied to the core on this computer; the core keeps its own"
        );
    }
}
