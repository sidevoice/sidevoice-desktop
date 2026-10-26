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
/// Decided only on a list read whole: `schtasks /FO CSV` quotes every field and writes one record per line, so each
/// non-empty line must be exactly one record of [`COLUMNS`] fields that, written back with every field quoted, gives
/// the line itself (the csv reader recovers from broken quoting; this check does not), and its first field must be a
/// task path (`\`, folders, a leaf, none empty). Anything else makes the whole answer unknown, and so does a list with
/// no task row (every account sees the system's own tasks).
fn seen_in(list: &[u8], name: &str) -> Seen {
    let prefix = format!("{}-", name.to_ascii_lowercase());
    let list = list.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(list);
    let mut rows = 0;
    let mut present = false;
    for line in list.split(|&byte| byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some(record) = strict_record(line) else { return Seen::Unknown };
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

/// The one record of [`COLUMNS`] fields in `line`, only when writing it back with every field quoted gives `line`.
fn strict_record(line: &[u8]) -> Option<csv::ByteRecord> {
    let mut reader = csv::ReaderBuilder::new().has_headers(false).flexible(true).from_reader(line);
    let mut records = reader.byte_records();
    let record = records.next()?.ok()?;
    if records.next().is_some() || record.len() != COLUMNS {
        return None;
    }
    let mut writer = csv::WriterBuilder::new()
        .quote_style(csv::QuoteStyle::Always)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer.write_byte_record(&record).ok()?;
    let written = writer.into_inner().ok()?;
    (written.strip_suffix(b"\n")? == line).then_some(record)
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
        // A byte-order mark before the first record, and a last line with no terminator.
        assert_eq!(seen("\u{FEFF}\"\\sidevoice-core-a\",\"N/A\",\"Ready\"", "sidevoice-core"), Seen::Present);
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
            // Broken quoting the csv reader would recover from: an unterminated last field, junk after a closing
            // quote, a truncated last field, a field left unquoted.
            "\"\\Task\",\"N/A\",\"Ready\"\r\n\"\\Other\",\"N/A\",\"Rea",
            "\"\\Task\",\"N/A\",\"Ready\"\r\n\"\\Other\"junk,\"N/A\",\"Ready\"\r\n",
            "\"\\Task\",\"N/A\",\"Ready\"\r\n\"\\Other\",\"N/A\",\"Ready",
            "\"\\Task\",N/A,\"Ready\"\r\n",
            "\"\\Task\",\"N/A\",\"Ready\" \r\n",
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
