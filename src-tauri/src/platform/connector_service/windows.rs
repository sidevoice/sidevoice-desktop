//! Windows: Task Scheduler. A job is the per-user task `<name>-<user>` (sidevoice-connector
//! `platform/service/windows.rs`; task names are machine-wide). `schtasks /Query` of one name fails alike for a
//! missing task and for any other failure, so the job is looked for in the whole list this user can see, read through
//! the bounded runner: absent only when that list was read whole, every record in it is a well-formed task row, and no
//! task carries the job's name, for any user (the user name in the output may be in another code page). The running
//! programs come from the process list (the trait's default).

use super::{run, Os, Seen};
use std::path::Path;

/// The columns of `schtasks /Query /FO CSV /NH`: the task's path, its next run time and its status.
const COLUMNS: usize = 3;

pub(super) struct Native;

impl Os for Native {
    fn job(_home: &Path, _label: &str, name: &str) -> Seen {
        // `schtasks` from the system directory, which the process search looks in before any `PATH` entry.
        let Some(output) = run("schtasks", &["/Query", "/FO", "CSV", "/NH"]) else { return Seen::Unknown };
        if !output.status.success() {
            return Seen::Unknown;
        }
        seen_in(&output.stdout, name)
    }
}

/// What the CSV task list says of the job `name`: present when a task's leaf name is `<name>-…` in any folder.
/// Decided only on a list read whole: any record that is not CSV, has another number of fields, or whose first field
/// is not a task path (`\`, folders, a leaf, none empty) makes it unknown, and so does a list with no task row (every
/// account sees the system's own tasks).
fn seen_in(list: &[u8], name: &str) -> Seen {
    let prefix = format!("{}-", name.to_ascii_lowercase());
    let mut reader = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(list);
    let mut rows = 0;
    let mut present = false;
    for record in reader.byte_records() {
        let Ok(record) = record else { return Seen::Unknown };
        if record.len() != COLUMNS {
            return Seen::Unknown;
        }
        let Some(leaf) = task_leaf(&record[0]) else { return Seen::Unknown };
        rows += 1;
        present |= leaf.to_ascii_lowercase().starts_with(&prefix);
    }
    match (rows, present) {
        (0, _) => Seen::Unknown,
        (_, true) => Seen::Present,
        (_, false) => Seen::Absent,
    }
}

/// The leaf of a task path `\folder\…\leaf`, decoded; `None` when it is not one.
fn task_leaf(path: &[u8]) -> Option<String> {
    let path = String::from_utf8_lossy(path);
    let rest = path.strip_prefix('\\')?;
    let parts: Vec<&str> = rest.split('\\').collect();
    if parts.iter().any(|part| part.is_empty() || part.chars().any(char::is_control)) {
        return None;
    }
    parts.last().map(|leaf| (*leaf).to_owned())
}

/// The `schtasks` CSV list is a Windows tool's output: only Windows reads it.
#[cfg(test)]
mod tests {
    use super::*;

    fn seen(list: &str, name: &str) -> Seen {
        seen_in(list.as_bytes(), name)
    }

    #[test]
    fn a_task_list_shows_a_job_for_any_user_in_any_folder() {
        let list = "\r\n\"\\Microsoft\\Windows\\Defrag\\ScheduledDefrag\",\"N/A\",\"Ready\"\r\n\"\\Sidevoice-Core-Ana\",\"N/A\",\"Running\"\r\n";
        assert_eq!(seen(list, "sidevoice-core"), Seen::Present);
        assert_eq!(seen(list, "sidevoice-connector"), Seen::Absent);
        assert_eq!(seen("\"\\Folder\\sidevoice-connector-x\",\"N/A\",\"Ready\"", "sidevoice-connector"), Seen::Present);
        // Escaped quotes and commas are decoded, in folders and in other columns.
        assert_eq!(
            seen("\"\\Folder\"\"quoted\\sidevoice-core-alice\",\"N/A\",\"Ready\"\r\n", "sidevoice-core"),
            Seen::Present
        );
        assert_eq!(
            seen("\"\\A, B\\sidevoice-core-alice\",\"1/2/2026, 10:00\",\"Ready\"\r\n", "sidevoice-core"),
            Seen::Present
        );
        assert_eq!(seen("\"\\A, B\\other\",\"1/2/2026, 10:00\",\"Ready\"\r\n", "sidevoice-core"), Seen::Absent);
    }

    #[test]
    fn a_list_not_read_whole_is_never_absence() {
        for list in [
            "",
            "\r\n",
            "\"",
            "INFO: There are no scheduled tasks presently available at your access level.\r\n",
            // A row with another number of fields, alone or among valid rows.
            "\"\\Task\",\"N/A\"\r\n",
            "\"\\Task\",\"N/A\",\"Ready\"\r\n\"\\Other\",\"N/A\"\r\n",
            "\"\\Task\",\"N/A\",\"Ready\"\r\nINFO: something else\r\n",
            // Truncated mid-record.
            "\"\\Task\",\"N/A\",\"Ready\"\r\n\"\\Other\",\"N/",
            // Task names that are not task paths.
            "\"Task\",\"N/A\",\"Ready\"\r\n",
            "\"\\\",\"N/A\",\"Ready\"\r\n",
            "\"\\Folder\\\\Task\",\"N/A\",\"Ready\"\r\n",
            "\"\\Folder\\\",\"N/A\",\"Ready\"\r\n",
        ] {
            assert_eq!(seen(list, "sidevoice-core"), Seen::Unknown, "{list:?}");
        }
    }
}
