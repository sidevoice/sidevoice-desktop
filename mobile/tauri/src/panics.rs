//! The app's panics, kept past the crash they may end in. A panic the app cannot unwind from (in a callback from the
//! system, in a destructor while unwinding, on a thread nothing catches) aborts the process, and the system's crash
//! report then has only the stack: not the message, not the place. So a hook adds every panic to a small file beside the
//! log file; the next launch logs what that file holds (`target: "panic"`) as the last run's panics, and removes it.
//!
//! The hook never logs (the logging guideline's exception for a platform where it can deadlock): a panic inside a log call (a `Display` that panics) happens with the writer's lock held, and
//! logging from the hook, on the same thread, would wait for it forever. It only writes the file (and the standard hook,
//! which prints to standard error, still runs).

use std::fs;
use std::io::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The file, in the app's log folder.
const FILE: &str = "last-panic.txt";

/// The most the file holds: a run that panics over and over keeps its first panics, and writes no more.
const LIMIT: u64 = 16 * 1024;

/// The file's path, known once the log folder is; a panic before that is only printed.
static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Installs the hook, ahead of the standard hook (which still prints to standard error). Once: the app's entry runs again
/// when Android recreates the activity, and a second hook would wrap the first.
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let standard = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(path) = PATH.get() {
                keep(path, &describe(info));
            }
            standard(info);
        }));
    });
}

/// Logs the panics of the last run that `dir` holds and removes them; this run's go to the same place.
pub fn watch(dir: &Path) {
    let path = dir.join(FILE);
    match take(&path) {
        // One event per panic: the file holds the hook's one-line reports. The writer shows the home as `~`.
        Ok(Some(text)) => {
            for report in text.lines() {
                tracing::error!(target: "panic", report, "the last run panicked");
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(kind = ?error.kind(), os_error = error.raw_os_error(), "the panic file was not read")
        }
    }
    let _ = PATH.set(path);
}

/// A panic in one line: when, on which thread, where, and its message (`panicked at <file:line:column>:\n<message>`).
fn describe(info: &PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("<unnamed>");
    let when = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    format!("{when} (Unix time) thread '{name}' {info}").replace('\n', " ")
}

/// Adds `line` to the file at `path`, cut to what is left of [`LIMIT`] (nothing once it is full). Never panics: a panic in
/// the hook aborts.
fn keep(path: &Path, line: &str) {
    let held = fs::metadata(path).map_or(0, |file| file.len());
    // One byte is the newline.
    let room = usize::try_from(LIMIT.saturating_sub(held))
        .unwrap_or(0)
        .saturating_sub(1);
    if room == 0 {
        return;
    }
    let mut end = line.len().min(room);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", &line[..end]);
    }
}

/// What the file at `path` holds, which is removed: none when there is no file.
fn take(path: &Path) -> std::io::Result<Option<String>> {
    let text = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    fs::remove_file(path)?;
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panics_of_a_run_are_kept_once_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert_eq!(take(&path).unwrap(), None, "no run panicked");
        keep(&path, "first");
        keep(&path, "second");
        assert_eq!(take(&path).unwrap().as_deref(), Some("first\nsecond\n"));
        assert_eq!(take(&path).unwrap(), None, "taken once");
        let long = "é".repeat(LIMIT as usize);
        keep(&path, &long);
        keep(&path, "after the limit");
        let kept = take(&path).unwrap().unwrap();
        assert!(
            kept.len() as u64 <= LIMIT,
            "one line is cut to the limit, and nothing more is added once it is full"
        );
        assert!(!kept.contains("after"));
    }

    #[test]
    fn a_panic_is_one_line_with_its_thread_and_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let hook_path = path.clone();
        let standard = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| keep(&hook_path, &describe(info))));
        std::thread::Builder::new()
            .name("a-worker".into())
            .spawn(|| panic!("a message\nover two lines"))
            .unwrap()
            .join()
            .unwrap_err();
        std::panic::set_hook(standard);
        let kept = take(&path).unwrap().unwrap();
        assert_eq!(kept.lines().count(), 1, "{kept}");
        assert!(kept.contains("thread 'a-worker' panicked at "), "{kept}");
        assert!(kept.contains("panics.rs:"), "{kept}");
        assert!(kept.contains("a message over two lines"), "{kept}");
    }
}
