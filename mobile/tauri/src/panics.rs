//! The app's panics, kept past the crash they may end in. A panic the app cannot unwind from (in a callback from the
//! system, in a destructor while unwinding, on a thread nothing catches) aborts the process, and the system's crash
//! report then has only the stack: not the message, not the place. So every panic is logged by a hook (the app's log: the
//! unified log on iOS, logcat on Android, and its file) and added to a small file beside the log file; the next launch
//! logs what that file holds as the last run's panics, and removes it.

use std::fs;
use std::io::Write as _;
use std::panic::PanicHookInfo;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// The file, in the app's log folder.
const FILE: &str = "last-panic.txt";

/// The most the file holds: a run that panics over and over keeps its first panics, and writes no more.
const LIMIT: u64 = 16 * 1024;

/// Logs the panics of the last run that `dir` holds, removes them, and installs the hook that keeps this run's there, ahead
/// of the standard hook (which still prints to standard error).
pub fn watch(dir: &Path) {
    let path = dir.join(FILE);
    match take(&path) {
        Ok(Some(text)) => log::error!("the last run panicked:\n{}", text.trim_end()),
        Ok(None) => {}
        Err(error) => log::warn!("panics: {}: {error}", path.display()),
    }
    let standard = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let line = describe(info);
        // The file first: a panic in the log's own code would hold its lock.
        keep(&path, &line);
        log::error!("{line}");
        standard(info);
    }));
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

/// Adds `line` to the file at `path`, unless it holds [`LIMIT`] already. Never panics: a panic in the hook aborts.
fn keep(path: &Path, line: &str) {
    if fs::metadata(path).is_ok_and(|file| file.len() >= LIMIT) {
        return;
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
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
        let long = "x".repeat(LIMIT as usize);
        keep(&path, &long);
        keep(&path, "after the limit");
        let kept = take(&path).unwrap().unwrap();
        assert_eq!(kept.len(), long.len() + 1, "nothing more once it is full");
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
