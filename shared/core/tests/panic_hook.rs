//! The panic hook in a real process (logging guideline, sections 9 and 11): this test binary runs itself as a child that
//! installs the shared logging, with a file sink and a system-log layer, and panics on two threads, once with a literal
//! message and once with a formatted one that carries a canary path. The child's file, system-log lines and stderr are
//! scanned. Its own test binary and one test, because the hook and the subscriber are the process's.
#![allow(clippy::disallowed_macros)] // test output is not logging

use sidevoice_shared::logging::{self, Extras, Home, Resource};
use sidevoice_shared::system_log::SystemLog;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const CHILD: &str = "SIDEVOICE_PANIC_HOOK_CHILD";
const CANARY_HOME: &str = "/home/canary-user-7f3a";
const LITERAL: &str = "literal words that may be logged";

fn append(path: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    writeln!(file, "{line}").unwrap();
}

/// What the child does: log to `dir`, then panic twice, each on a thread of its own.
fn child(dir: &Path) {
    let (file, system) = (dir.join("app.log"), dir.join("system.log"));
    let home = Some(Path::new(CANARY_HOME));
    let layer = SystemLog::new(Home::new(home), move |_, _, line: &str| append(&system, line));
    logging::init_with(
        &["sidevoice_shared", logging::PANIC_TARGET, logging::BACKTRACE_TARGET],
        home,
        move |line| append(&file, line),
        Extras { layer: Some(layer), stderr: false, panic_hook: true },
    );
    logging::announce(Resource::of("canary-app", "9.9.9", "macos", "aarch64"));
    let literal = std::thread::Builder::new()
        .name("literal-panic".into())
        .spawn(|| panic!("literal words that may be logged"))
        .unwrap();
    assert!(literal.join().is_err());
    let formatted = std::thread::Builder::new()
        .name("formatted-panic".into())
        .spawn(|| {
            let secret = format!("{CANARY_HOME}/canary-secret-7f3a");
            let result: Result<(), String> = std::hint::black_box(Err(secret));
            result.unwrap();
        })
        .unwrap();
    assert!(formatted.join().is_err());
}

#[test]
fn a_panic_is_two_redacted_events_and_the_backtrace_reaches_the_file_only() {
    if let Ok(dir) = std::env::var(CHILD) {
        return child(Path::new(&dir));
    }
    let scratch = std::env::temp_dir().join(format!("sidevoice-panic-hook-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let stderr_path = scratch.join("stderr.txt");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_panic_is_two_redacted_events_and_the_backtrace_reaches_the_file_only", "--nocapture"])
        .env(CHILD, &scratch)
        .env("SIDEVOICE_LOG", "trace")
        .env("HOME", CANARY_HOME)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .status()
        .unwrap();
    assert!(status.success(), "the child ran to its end");
    let file = std::fs::read_to_string(scratch.join("app.log")).unwrap();
    let system = std::fs::read_to_string(scratch.join("system.log")).unwrap_or_default();
    let stderr = std::fs::read_to_string(&stderr_path).unwrap();
    let _ = std::fs::remove_dir_all(&scratch);

    // The start event: the resource, in OpenTelemetry's values.
    let start = file.lines().find(|line| line.contains("the application started")).expect(&file);
    for field in [
        "service.name=\"canary-app\"",
        "service.version=\"9.9.9\"",
        "os.type=\"darwin\"",
        "os.name=\"macOS\"",
        "host.arch=\"arm64\"",
    ] {
        assert!(start.contains(field), "{field} missing: {start}");
    }

    let panics: Vec<&str> = file.lines().filter(|line| line.contains(" panic: ")).collect();
    let backtraces: Vec<&str> = file.lines().filter(|line| line.contains(" crash_backtrace: ")).collect();
    assert_eq!((panics.len(), backtraces.len()), (2, 2), "{file}");
    for line in &panics {
        for field in [
            "exception.type=\"panic\"",
            "code.file.path=",
            "code.line.number=",
            "thread.name=",
            "service.version=\"9.9.9\"",
        ] {
            assert!(line.contains(field), "{field} missing: {line}");
        }
        assert!(!line.contains("exception.stacktrace"), "{line}");
    }
    let literal = panics.iter().find(|line| line.contains("literal-panic")).unwrap();
    assert!(literal.contains(&format!("exception.message=\"{LITERAL}\"")), "{literal}");
    assert!(!literal.contains("len="), "{literal}");
    let formatted = panics.iter().find(|line| line.contains("formatted-panic")).unwrap();
    assert!(formatted.contains("exception.message=\"<formatted panic message omitted>\""), "{formatted}");
    assert!(formatted.contains("len="), "{formatted}");
    for line in &backtraces {
        assert!(line.contains("exception.stacktrace=Backtrace ["), "{line}");
    }

    // The system log gets the panic record and never the backtrace (guideline section 8).
    assert!(system.contains("the program panicked"), "{system}");
    assert!(
        !system.contains("crash_backtrace") && !system.contains("stacktrace") && !system.contains("Backtrace"),
        "{system}"
    );
    // Stderr: the default hook is not chained, and the place of the panic, no message, is all it carries.
    assert!(
        !stderr.contains("thread '") && !stderr.contains("crash_backtrace") && !stderr.contains("stacktrace"),
        "{stderr}"
    );
    assert!(stderr.contains("sidevoice: panic at "), "{stderr}");
    assert!(!stderr.contains(LITERAL), "{stderr}");

    for text in [&file, &system, &stderr] {
        for forbidden in ["canary-secret-7f3a", "canary-user-7f3a"] {
            assert!(!text.contains(forbidden), "{forbidden} leaked:\n{text}");
        }
    }
}
