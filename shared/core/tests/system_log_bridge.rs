//! The phone's system log through the real `log` bridge (logging guideline, sections 8 and 11): its own test binary, one test and the
//! global subscriber installed the way the mobile app does (`init_with`, the system layer as the extra layer, `SIDEVOICE_LOG` wide
//! open). A dependency's record, the page's, an info line of ours and a backtrace never reach it; a warning of ours does, with the
//! home directory shown as `~`.

use std::sync::{Arc, Mutex};

use sidevoice_shared::logging::{init_with, Extras, Home};
use sidevoice_shared::system_log::SystemLog;

const HOME: &str = "/home/canary-user-7f3a";

#[test]
fn the_system_log_takes_only_warnings_of_our_targets() {
    std::env::set_var("SIDEVOICE_LOG", "trace");
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&seen);
    let home = Some(std::path::Path::new(HOME));
    let layer = SystemLog::new(Home::new(home), move |_, line: &str| sink.lock().unwrap().push(line.to_owned()));
    let own = ["sidevoice_mobile_lib", "webview", "stills", "crash_backtrace"];
    let system = ["sidevoice_mobile_lib", "stills"];
    init_with(
        &own,
        home,
        |_| {},
        Extras { layer: Some(layer), system_targets: &system, stderr: false, panic_hook: false },
    );

    tracing::info!(target: "stills", "an info line");
    tracing::warn!(target: "stills", "a warning of ours {HOME}/x");
    tracing::error!(target: "crash_backtrace", "frames");
    for (target, level) in [
        ("tao", log::Level::Error),
        ("webview", log::Level::Error),
        ("stills", log::Level::Warn),
        ("stills", log::Level::Info),
    ] {
        let record = log::Record::builder().args(format_args!("a record")).level(level).target(target).build();
        tracing_log::format_trace(&record).unwrap();
    }

    let seen = seen.lock().unwrap();
    assert_eq!(*seen, ["a warning of ours ~/x", "a record"], "{seen:?}");
}
