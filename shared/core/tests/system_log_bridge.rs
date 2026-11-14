//! The phone's system log through the real `log` bridge (logging guideline, sections 6 and 9): its own test binary, one test and the
//! global subscriber installed the way the mobile app does (`init_with`, the system layer as the extra layer). Every public site of the
//! app is a `log::` macro, so a public category is reached only if the bridged record's real target is read back; a canary home path
//! must show as `~`, and a page's line must be private.

use std::sync::{Arc, Mutex};

use sidevoice_shared::logging::{init_with, Extras, Home};
use sidevoice_shared::system_log::SystemLog;

const HOME: &str = "/home/canary-user-7f3a";

#[test]
fn log_records_reach_the_category_of_their_real_target() {
    std::env::set_var("SIDEVOICE_LOG", "info");
    let seen = Arc::new(Mutex::new(Vec::<(&'static str, String)>::new()));
    let sink = Arc::clone(&seen);
    let home = Some(std::path::Path::new(HOME));
    let layer = SystemLog::new(Home::new(home), move |category, _, line: &str| {
        sink.lock().unwrap().push((category, line.to_owned()))
    });
    let own = ["sidevoice_mobile_lib", "webview", "stills", "intent", "cue", "audio"];
    init_with(&own, home, |_| {}, Extras { layer: Some(layer), stderr: false, panic_hook: false });

    for target in ["stills", "intent", "cue", "audio"] {
        log::info!(target: target, "public {target} {HOME}/x");
    }
    log::info!(target: "webview", "console.warn: [sidevoice] call surface: no still for canary");
    log::info!(target: "sidevoice_mobile_lib::call", "an app line");
    tracing::warn!("a native line");

    let seen = seen.lock().unwrap();
    let categories: Vec<_> = seen.iter().map(|(category, _)| *category).collect();
    assert_eq!(categories, ["stills", "intent", "cue", "audio", "app", "app", "app"], "{seen:?}");
    assert!(seen.iter().all(|(_, line)| !line.contains("canary-user")), "{seen:?}");
    assert!(seen[0].1.contains("public stills ~/x"), "{seen:?}");
}
