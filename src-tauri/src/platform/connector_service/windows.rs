//! Windows: Task Scheduler. A job is the per-user task `<name>-<user>` (sidevoice-connector
//! `platform/service/windows.rs`; task names are machine-wide). `schtasks /Query` of one name fails alike for a
//! missing task and for any other failure, so the job is looked for in the whole list this user can see: absent only
//! when that list was read and no task in it carries the job's name, for any user (the user name in the output may be
//! in another code page). The running programs come from the process list (the trait's default).

use super::{Os, Seen};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub(super) struct Native;

impl Os for Native {
    fn job(_home: &Path, _label: &str, name: &str) -> Seen {
        // `schtasks` from the system directory, which the process search looks in before any `PATH` entry; no
        // console window for it.
        let Ok(output) =
            Command::new("schtasks").args(["/Query", "/FO", "CSV", "/NH"]).creation_flags(CREATE_NO_WINDOW).output()
        else {
            return Seen::Unknown;
        };
        if !output.status.success() {
            return Seen::Unknown;
        }
        seen_in(&String::from_utf8_lossy(&output.stdout), name)
    }
}

/// Whether the CSV task list has a task named `<name>-…` in any folder. A list with no task row was not read (every
/// account sees the system's own tasks).
fn seen_in(list: &str, name: &str) -> Seen {
    let prefix = format!("{}-", name.to_ascii_lowercase());
    let mut rows = 0;
    for line in list.lines().filter(|line| line.starts_with('"')) {
        rows += 1;
        let task = line[1..].split('"').next().unwrap_or_default();
        let leaf = task.rsplit('\\').next().unwrap_or_default().to_ascii_lowercase();
        if leaf.starts_with(&prefix) {
            return Seen::Present;
        }
    }
    if rows == 0 {
        Seen::Unknown
    } else {
        Seen::Absent
    }
}

/// The `schtasks` CSV list is a Windows tool's output: only Windows reads it.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_list_shows_a_job_for_any_user_and_never_absence_without_rows() {
        let list = "\r\n\"\\Microsoft\\Windows\\Defrag\\ScheduledDefrag\",\"N/A\",\"Ready\"\r\n\"\\Sidevoice-Core-Ana\",\"N/A\",\"Running\"\r\n";
        assert_eq!(seen_in(list, "sidevoice-core"), Seen::Present);
        assert_eq!(seen_in(list, "sidevoice-connector"), Seen::Absent);
        assert_eq!(
            seen_in("\"\\Folder\\sidevoice-connector-x\",\"N/A\",\"Ready\"", "sidevoice-connector"),
            Seen::Present
        );
        assert_eq!(
            seen_in(
                "INFO: There are no scheduled tasks presently available at your access level.\r\n",
                "sidevoice-core"
            ),
            Seen::Unknown
        );
        assert_eq!(seen_in("", "sidevoice-core"), Seen::Unknown);
    }
}
