//! Whether the connector's service is on this computer, asked fresh of the OS: the jobs its service manager has for
//! this user and the programs running now. The reset (`data.rs`) asks this when the connector's install record is
//! missing: a missing record alone is never proof that the service is gone.
//!
//! The connector defines two jobs (sidevoice-connector `service/jobs.rs`: `dev.sidevoice.core` /
//! `sidevoice-core` and `dev.sidevoice.connector` / `sidevoice-connector`) with this user's service manager:
//! launchd on macOS, systemd's user manager on Linux, Task Scheduler on Windows. Each OS file reports what its
//! manager says of one job and which programs run; the decision is here, and it is fail-closed: the service is
//! absent only when every job is reported absent and no managed program runs. A manager that cannot be asked, an
//! answer that cannot be read, or a process list that cannot be read is never absence.

#[cfg_attr(target_os = "macos", path = "connector_service/macos.rs")]
#[cfg_attr(target_os = "windows", path = "connector_service/windows.rs")]
#[cfg_attr(target_os = "linux", path = "connector_service/linux.rs")]
mod imp;
use imp::Native;

use std::path::Path;
use std::time::Duration;

/// How long a service manager or process-list command may take: the connector's own deadline for its manager
/// commands (sidevoice-connector `platform/service.rs`, `MANAGER_LIMIT`).
const COMMAND_LIMIT: Duration = Duration::from_secs(30);
/// How long its output may take to end once it has exited.
const PIPE_GRACE: Duration = Duration::from_secs(2);
/// The most output kept per stream; more is an answer that was not read whole.
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

/// The connector's jobs (sidevoice-connector `service/jobs.rs`), by the names each service manager derives what it
/// calls them from: the reverse-DNS label (launchd) and the short name (the systemd unit `<name>.service`, the Windows
/// task `<name>-<user>`).
const JOBS: [(&str, &str); 2] =
    [("dev.sidevoice.core", "sidevoice-core"), ("dev.sidevoice.connector", "sidevoice-connector")];

/// The programs the connector's service runs (the core's `sidevoice-core-rust`, the connector, and on Windows the
/// windowless starter `sidevoice-background`), plus `sidevoice-core` as older installs name it.
const PROGRAMS: [&str; 4] = ["sidevoice-core-rust", "sidevoice-core", "sidevoice-connector", "sidevoice-background"];

/// The longest name some systems keep for a process (Linux's `comm`, 15 bytes): a name of exactly that length that
/// starts a managed program's name is taken for it.
const SHORT_NAME: usize = 15;

/// What a service manager says of one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// The manager has it: defined, loaded or registered.
    Present,
    /// The manager answered, and it has no such job.
    Absent,
    /// The manager could not be asked, or its answer could not be read. Never absence.
    Unknown,
}

/// Fresh proof that the connector's service is not on this computer for the account home `home`: every job reported
/// absent by this user's service manager, and none of its programs running. False on any doubt.
pub fn absent(home: &Path) -> bool {
    absent_with(home, Native::job, Native::programs)
}

fn absent_with(
    home: &Path,
    job: impl Fn(&Path, &str, &str) -> Seen,
    programs: impl FnOnce() -> Option<Vec<String>>,
) -> bool {
    // Only the canonical home: a link or a relative path could point the manager's files elsewhere.
    if dunce::canonicalize(home).ok().as_deref() != Some(home) {
        return false;
    }
    if !JOBS.iter().all(|(label, name)| job(home, label, name) == Seen::Absent) {
        return false;
    }
    programs().is_some_and(|running| !running.iter().any(|name| managed(name)))
}

/// Whether a running program, as the OS names it (a path or a file name), is one of the connector's.
fn managed(reported: &str) -> bool {
    let Some(name) = Path::new(reported.trim()).file_name().and_then(|name| name.to_str()) else { return false };
    let name = name.to_ascii_lowercase();
    let name = name.strip_suffix(std::env::consts::EXE_SUFFIX).unwrap_or(&name);
    PROGRAMS.iter().any(|program| name == *program || (name.len() == SHORT_NAME && program.starts_with(name)))
}

/// Run `program` with `args` through the shared bounded runner (`sidevoice-platform` `process`: its own process
/// group or Job Object, no console window, killed and reaped on any exit from the wait), at most [`COMMAND_LIMIT`].
/// `None` when it could not be started, missed its deadline, or its output was cut short or did not end: an answer
/// that was not read whole is never one that says "absent". Runs on a thread of its own with its own runtime, so it
/// can be called from any context.
fn run(program: &str, args: &[&str]) -> Option<sidevoice_platform::process::Output> {
    run_within(COMMAND_LIMIT, program, args)
}

fn run_within(limit: Duration, program: &str, args: &[&str]) -> Option<sidevoice_platform::process::Output> {
    use sidevoice_platform::process::{Process, Processes};
    use std::process::Stdio;
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        runtime.block_on(async move {
            let child = Processes::spawn_tree(command).await.ok()?;
            let output = child.output(limit, PIPE_GRACE, OUTPUT_LIMIT).await.ok()?;
            (output.complete && !output.truncated).then_some(output)
        })
    })
    .join()
    .ok()
    .flatten()
}

/// What differs per OS. A host whose manager this module cannot ask keeps the defaults: the service is never proven
/// absent there.
trait Os {
    /// What this user's service manager says of the job `label` / `name`, for the account home `home`.
    fn job(_home: &Path, _label: &str, _name: &str) -> Seen {
        Seen::Unknown
    }

    /// The programs running now, by every name the OS gives each (its name, its executable, its first argument);
    /// `None` when they cannot be listed. Every process on the computer, whoever runs it.
    fn programs() -> Option<Vec<String>> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always).with_cmd(UpdateKind::Always),
        );
        // A list without this very process was not read: an empty or partial list is not "nothing runs".
        system.process(sysinfo::Pid::from_u32(std::process::id()))?;
        let mut names = Vec::new();
        for process in system.processes().values() {
            names.push(process.name().to_string_lossy().into_owned());
            if let Some(exe) = process.exe() {
                names.push(exe.to_string_lossy().into_owned());
            }
            if let Some(first) = process.cmd().first() {
                names.push(first.to_string_lossy().into_owned());
            }
        }
        Some(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let home = dunce::canonicalize(temp.path()).unwrap();
        (temp, home)
    }

    #[test]
    fn only_every_job_absent_and_nothing_running_is_absence() {
        let (_temp, home) = home();
        let nothing = || Some(vec!["/usr/bin/other".to_owned()]);
        assert!(absent_with(&home, |_, _, _| Seen::Absent, nothing));
        assert!(!absent_with(&home, |_, _, _| Seen::Unknown, nothing));
        assert!(!absent_with(&home, |_, _, _| Seen::Present, nothing));
        assert!(!absent_with(
            &home,
            |_, _, name| if name == "sidevoice-connector" { Seen::Present } else { Seen::Absent },
            nothing
        ));
        assert!(!absent_with(
            &home,
            |_, _, name| if name == "sidevoice-core" { Seen::Unknown } else { Seen::Absent },
            nothing
        ));
        // A process list that could not be read is not an empty one.
        assert!(!absent_with(&home, |_, _, _| Seen::Absent, || None));
        assert!(!absent_with(&home, |_, _, _| Seen::Absent, || Some(vec!["sidevoice-connector".to_owned()])));
    }

    #[test]
    fn a_home_that_is_not_canonical_is_never_proven_free() {
        let (_temp, home) = home();
        assert!(!absent_with(&home.join("missing"), |_, _, _| Seen::Absent, || Some(vec![])));
        assert!(!absent_with(Path::new("relative"), |_, _, _| Seen::Absent, || Some(vec![])));
    }

    #[test]
    fn the_connectors_programs_are_recognised_by_any_name_the_os_gives() {
        let exe = std::env::consts::EXE_SUFFIX;
        let releases = Path::new("Sidevoice").join("install").join("releases").join("1.2.3");
        for program in [
            releases.join("core").join("bin").join(format!("sidevoice-core-rust{exe}")),
            releases.join("bin").join(format!("sidevoice-connector{exe}")),
            releases.join("bin").join(format!("sidevoice-background{exe}")),
            Path::new("sidevoice-core").to_path_buf(),
        ] {
            assert!(managed(&program.to_string_lossy()), "{}", program.display());
        }
        // Cut short by the OS (Linux `comm`), or in another case.
        assert!(managed("sidevoice-core-"));
        assert!(managed("sidevoice-conne"));
        assert!(managed(&format!("SIDEVOICE-CONNECTOR{exe}")));
        assert!(!managed("/usr/bin/other-service"));
        assert!(!managed("sidevoice"));
        assert!(!managed("sidevoice-desktop"));
        assert!(!managed(""));
    }

    /// Not a check: the slow command the deadline test runs (this test binary, asked for this test alone).
    #[test]
    #[ignore]
    fn sleeper() {
        std::thread::sleep(Duration::from_secs(20));
    }

    #[test]
    fn a_command_past_its_deadline_is_no_answer() {
        let program = std::env::current_exe().unwrap();
        let program = program.to_str().unwrap();
        let listed = run_within(Duration::from_secs(20), program, &["--list"]).expect("a quick command answers");
        assert!(listed.status.success());
        let started = std::time::Instant::now();
        let slow = ["--ignored", "--exact", "platform::connector_service::tests::sleeper"];
        assert!(run_within(Duration::from_millis(500), program, &slow).is_none());
        assert!(started.elapsed() < Duration::from_secs(10), "the slow command was killed, not waited for");
        assert!(run("/nonexistent/sidevoice-no-such-program", &[]).is_none());
    }

    /// The real observation on this host: a test run never proves a service exists, but every answer it gives must be
    /// one of the three, and the process list (where read) includes this test.
    #[test]
    fn the_hosts_own_observation_is_well_formed() {
        let (_temp, home) = home();
        for (label, name) in JOBS {
            let _ = Native::job(&home, label, name);
        }
        if let Some(running) = Native::programs() {
            assert!(!running.is_empty());
        }
    }
}
