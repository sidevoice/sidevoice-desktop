//! Crash reports on the desktop (sidevoice-shared `crash`): the reporter, the panic hook that feeds it, and the copy of the app's
//! switch to the core this app installed on this computer.
//!
//! The setting is the person's (Settings → Crash reports, `host.crashReports`), kept in the app's data folder and held to before
//! any page has loaded. The DSN is baked in by a release or nightly build (`SIDEVOICE_SENTRY_DSN`, set by CI); a local or dev
//! build has none and records and sends nothing, whatever the setting says. The app reports its own panics only: it reads,
//! relays and acknowledges nothing of the core's or the connector's (each piece sends its own).
//!
//! The switch goes one way, from the app to the core it installed (the logging guideline, section 10), through the core's
//! local-only endpoint on its socket. A copy is **owed** (`Reporter::copy_owed`, kept in a file of its own) from the app's own
//! install of the core and from each change of the switch the person makes afterwards, and stays owed until the core answers a
//! copy with a 200. While it is owed, the app tries once each time the local host sees a core answer after none did
//! ([`core_up`]), so a core that was not there, or was too old to have the endpoint, gets the person's choice when it is. Never
//! on a start that owes nothing, never when an update replaces the binaries, never read back: a change made on the core after
//! a delivered copy stands. A core the person installed by hand is not touched; its switch is its own.

use std::sync::Mutex;

use sidevoice_shared::crash::{self, Config, Mode, Reporter};
use sidevoice_shared::logging;
use sidevoice_shared_tauri::CrashReports;
use tauri::{AppHandle, Manager};

/// The DSN of a release or nightly build; empty or absent otherwise.
const DSN: Option<&str> = option_env!("SIDEVOICE_SENTRY_DSN");

/// The one way a copy is sent: one at a time, each sending the reporter's mode as it is when its turn comes (the latest wins).
static COPYING: Mutex<()> = Mutex::new(());

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
    reporter.on_copy_owed(move || copy_to_core(&handle));
    app.manage(CrashReports(reporter));
}

/// The app's own install has just put a core on this computer (its first: `LocalHost::on_installed`): a copy of the switch is
/// owed to it from now on, and goes now.
pub fn core_installed(app: &AppHandle) {
    if let Some(reports) = app.try_state::<CrashReports>() {
        reports.0.local_core_installed();
    }
}

/// A core answers on this computer after none did (`LocalHost::on_core_up`; the app's start with the core running is one):
/// the copy it is owed, if any, goes once.
pub fn core_up(app: &AppHandle) {
    if app.try_state::<CrashReports>().is_some_and(|reports| reports.0.copy_owed().is_some()) {
        copy_to_core(app);
    }
}

/// Sends what the core is owed, on a thread of its own, once: the person's click and the install's answer never wait for a
/// core. A core that cannot be reached, or that answers anything but a 200, is one warning and the copy stays owed.
fn copy_to_core(app: &AppHandle) {
    let app = app.clone();
    let started = std::thread::Builder::new().name("crash-switch".into()).spawn(move || {
        let _one_at_a_time = COPYING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(reporter) = app.try_state::<CrashReports>().map(|reports| reports.0.clone()) else { return };
        // Asked once the turn has come: a change made while another copy was on its way is sent, not the one before it.
        let Some(owed) = reporter.copy_owed() else { return };
        let outcome = match crate::local_host::running(&app) {
            Some(host) => host.set_crash_reports(owed.mode == Mode::Auto).map_err(|refusal| refusal.key),
            None => Err("local-host.off".to_owned()),
        };
        match outcome {
            Ok(()) => reporter.copy_delivered(owed),
            Err(reason) => tracing::warn!(
                error.type = crate::error::DesktopError::CrashSwitchNotCopied.code(),
                reason = logging::checked_code(&reason),
                "the crash report switch could not be copied to the core on this computer; it is tried again when a core next answers"
            ),
        }
    });
    if let Err(error) = started {
        tracing::warn!(
            error.type = crate::error::DesktopError::CrashSwitchNotCopied.code(),
            kind = ?error.kind(),
            "the crash report switch could not be copied to the core on this computer; it is tried again when a core next answers"
        );
    }
}
