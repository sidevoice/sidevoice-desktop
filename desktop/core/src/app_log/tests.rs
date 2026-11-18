//! The app log's pages over real files, rotated as tauri-plugin-log rotates them (`KeepSome(5)`, dated names): the same
//! behaviours the core's reader of its own log is held to (sidevoice-core, rust/server/logs/tests.rs).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::{hold, pages, read, read_waiting, Page, Refused};
use sidevoice_platform::fs::{Files, Fs};

const NAME: &str = "sidevoice-desktop";

fn append(path: &Path, text: &str) {
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

/// What the plugin does when the log is full: only the newest four dated files stay, then the log takes a dated name.
fn rotate(log: &Path) {
    static SECOND: AtomicU32 = AtomicU32::new(0);
    let dir = log.parent().unwrap();
    let mut dated: Vec<PathBuf> = dated_files(dir);
    dated.sort();
    while dated.len() > 4 {
        fs::remove_file(dated.remove(0)).unwrap();
    }
    let second = SECOND.fetch_add(1, Ordering::Relaxed);
    let name = format!("{NAME}_2026-11-18_10-{:02}-{:02}.log", second / 60, second % 60);
    fs::rename(log, dir.join(name)).unwrap();
}

fn dated_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_str().unwrap().starts_with("sidevoice-desktop_"))
        .collect()
}

fn texts(page: &Page) -> Vec<&str> {
    page.lines.iter().map(|line| line.text.as_str()).collect()
}

/// The log's newest file, in a folder that lives as long as the guard.
fn log() -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let log = temp.path().join(format!("{NAME}.log"));
    (temp, log)
}

fn numbered(from: usize, to: usize) -> String {
    (from..to).map(|n| format!("line {n}\n")).collect()
}

// --- The reader ---------------------------------------------------------------------------------

#[test]
fn an_empty_log_answers_with_the_start_cursor_and_no_line() {
    let (_temp, log) = log();
    let page = read(&log, None, 10).unwrap();
    assert!(page.lines.is_empty() && !page.more);
    assert_eq!(page.cursor, "start");
    append(&log, "");
    let page = read(&log, Some("start"), 10).unwrap();
    assert!(page.lines.is_empty());
    assert_eq!(page.cursor, "start");
}

#[test]
fn the_tail_spans_the_rotated_generations_and_ends_at_the_end() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 3));
    rotate(&log);
    append(&log, &numbered(3, 5));
    let page = read(&log, None, 4).unwrap();
    assert_eq!(texts(&page), ["line 1", "line 2", "line 3", "line 4"]);
    assert!(!page.more);
    assert_eq!(read(&log, None, 1000).unwrap().lines.len(), 5);
    assert!(read(&log, Some(&page.cursor), 10).unwrap().lines.is_empty());
    append(&log, "line 5\n");
    let next = read(&log, Some(&page.cursor), 10).unwrap();
    assert_eq!(texts(&next), ["line 5"]);
}

#[test]
fn the_tail_of_a_big_log_is_whole_lines_within_the_page_size() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 20_000));
    let page = read(&log, None, 1000).unwrap();
    assert_eq!(page.lines.len(), 1000);
    assert_eq!(page.lines[0].text, "line 19000");
    assert_eq!(page.lines[999].text, "line 19999");
    let long = "x".repeat(1000);
    let (_temp, log) = self::log();
    append(&log, &format!("{long}\n").repeat(2000));
    let page = read(&log, None, 1000).unwrap();
    assert!((200..300).contains(&page.lines.len()), "{}", page.lines.len());
    assert!(page.lines.iter().all(|line| line.text == long));
}

#[test]
fn a_page_is_at_most_limit_lines_and_the_cursor_pages_through_without_loss_or_repeat() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 10));
    let mut seen = Vec::new();
    let mut after = Some("start".to_owned());
    let mut pages = 0;
    loop {
        let page = read(&log, after.as_deref(), 4).unwrap();
        assert!(page.lines.len() <= 4);
        seen.extend(page.lines.iter().map(|line| line.text.clone()));
        pages += 1;
        after = Some(page.cursor);
        if !page.more {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, (0..10).map(|n| format!("line {n}")).collect::<Vec<_>>());
}

#[test]
fn a_line_has_one_id_however_it_is_read_again() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 6));
    let first = read(&log, Some("start"), 1000).unwrap();
    let tail = read(&log, None, 1000).unwrap();
    let ids: Vec<_> = first.lines.iter().map(|line| &line.id).collect();
    assert_eq!(ids, tail.lines.iter().map(|line| &line.id).collect::<Vec<_>>());
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 6);
    // It stays when the log rotates under it, and a cursor and an id are the same short, URL-safe thing.
    rotate(&log);
    append(&log, "line 6\n");
    let again = read(&log, Some("start"), 1000).unwrap();
    assert_eq!(again.lines[..6].iter().map(|line| &line.id).collect::<Vec<_>>(), ids);
    for text in again.lines.iter().map(|line| &line.id).chain([&again.cursor]) {
        assert!(text.len() <= 128);
        assert!(text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'));
    }
}

#[test]
fn a_cursor_goes_on_through_the_generations_after_a_rotation_without_loss_or_repeat() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 3));
    let cursor = read(&log, None, 100).unwrap().cursor;
    append(&log, &numbered(3, 5));
    rotate(&log);
    append(&log, &numbered(5, 8));
    let page = read(&log, Some(&cursor), 100).unwrap();
    assert_eq!(texts(&page), ["line 3", "line 4", "line 5", "line 6", "line 7"]);
    // Once more, with the cursor still in the first generation: now the second older one.
    rotate(&log);
    append(&log, &numbered(8, 9));
    let page = read(&log, Some(&cursor), 100).unwrap();
    assert_eq!(texts(&page).len(), 6);
    assert_eq!(page.lines[0].text, "line 3");
    assert_eq!(page.lines[5].text, "line 8");
    // A page that ends exactly where a generation ends goes on in the next.
    let ends = read(&log, Some(&cursor), 2).unwrap();
    assert_eq!(texts(&ends), ["line 3", "line 4"]);
    assert!(ends.more);
    let rest = read(&log, Some(&ends.cursor), 100).unwrap();
    assert_eq!(texts(&rest), ["line 5", "line 6", "line 7", "line 8"]);
    assert!(!rest.more);
}

#[test]
fn a_file_seen_under_two_names_is_one_generation_and_a_log_between_rotations_is_continuous() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 3));
    // A rotation between two opens shows one file twice.
    let twice = vec![Files::open_read(&log).unwrap(), Files::open_read(&log).unwrap()];
    assert_eq!(texts(&pages(twice, Some("start"), 10).unwrap()).len(), 3);
    // Renamed, and the new log not yet there.
    rotate(&log);
    let page = read(&log, None, 10).unwrap();
    assert_eq!(texts(&page), ["line 0", "line 1", "line 2"]);
    assert!(read(&log, Some(&page.cursor), 10).unwrap().lines.is_empty());
    append(&log, "line 3\n");
    let next = read(&log, Some(&page.cursor), 10).unwrap();
    assert_eq!(texts(&next), ["line 3"]);
}

#[test]
fn a_cursor_whose_generation_was_deleted_is_expired() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 3));
    let cursor = read(&log, None, 100).unwrap().cursor;
    // Five dated files stay, so the sixth rotation takes the first generation away.
    for round in 0..6 {
        rotate(&log);
        append(&log, &numbered(10 * (round + 1), 10 * (round + 1) + 2));
        let held = read(&log, Some(&cursor), 100);
        assert_eq!(held.is_ok(), round < 5, "after {} rotations", round + 1);
    }
    assert_eq!(read(&log, Some(&cursor), 100).err(), Some(Refused::Expired));
    // And one that was never there.
    assert_eq!(read(&log, Some("0123456789abcdef.0"), 100).err(), Some(Refused::Expired));
}

#[test]
fn a_cursor_past_the_end_of_its_file_is_expired() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 3));
    let (id, offset) = read(&log, None, 100)
        .unwrap()
        .cursor
        .split_once('.')
        .map(|(i, o)| (i.to_owned(), o.parse::<u64>().unwrap()))
        .unwrap();
    for past in [offset + 1, offset + 100] {
        assert_eq!(read(&log, Some(&format!("{id}.{past}")), 10).err(), Some(Refused::Expired));
    }
    // Where it ends, and where it starts, are fine.
    assert!(read(&log, Some(&format!("{id}.{offset}")), 10).unwrap().lines.is_empty());
    assert_eq!(read(&log, Some(&format!("{id}.0")), 10).unwrap().lines.len(), 3);
}

#[test]
fn a_malformed_cursor_is_invalid() {
    let (_temp, log) = log();
    append(&log, "alpha\nbeta\n");
    let valid = read(&log, None, 10).unwrap().cursor;
    let id = valid.split_once('.').unwrap().0;
    let too_long = format!("{id}.{}", "1".repeat(130));
    for bad in [
        "",
        "x",
        "start.",
        "start0",
        ".",
        "0123456789abcdef",
        "0123456789abcdef.",
        "0123456789abcde.1",
        "0123456789abcdefg.1",
        "zzzzzzzzzzzzzzzz.1",
        "0123456789abcdef.-1",
        "0123456789abcdef.+1",
        "0123456789abcdef.1.2",
        "0123456789abcdef.1x",
        "../../etc/passwd",
        &too_long,
        // Right generation, but in the middle of a line.
        &format!("{id}.3"),
    ] {
        assert_eq!(read(&log, Some(bad), 10).err(), Some(Refused::Invalid), "{bad:?}");
    }
}

#[test]
fn a_torn_last_line_is_never_delivered_and_is_whole_when_it_is_finished() {
    let (_temp, log) = log();
    append(&log, "alpha\nbeta\ngam");
    let page = read(&log, None, 10).unwrap();
    assert_eq!(texts(&page), ["alpha", "beta"]);
    assert_eq!(texts(&read(&log, Some("start"), 10).unwrap()), ["alpha", "beta"]);
    assert!(read(&log, Some(&page.cursor), 10).unwrap().lines.is_empty());
    append(&log, "ma\ndelta\n");
    let next = read(&log, Some(&page.cursor), 10).unwrap();
    assert_eq!(texts(&next), ["gamma", "delta"]);
    // A generation that is only a torn line has nothing to give yet.
    let (_temp, log) = self::log();
    append(&log, "never ended");
    let page = read(&log, None, 10).unwrap();
    assert!(page.lines.is_empty());
    assert_eq!(page.cursor, "start");
}

#[test]
fn a_page_stops_at_its_size_but_always_holds_a_whole_line() {
    let (_temp, log) = log();
    let big = "b".repeat(100_000);
    append(&log, &format!("{big}\n").repeat(5));
    let mut sizes = Vec::new();
    let mut after = "start".to_owned();
    loop {
        let page = read(&log, Some(&after), 1000).unwrap();
        sizes.push(page.lines.len());
        after = page.cursor;
        if !page.more {
            break;
        }
    }
    assert_eq!(sizes, [2, 2, 1]);
    let (_temp, log) = self::log();
    let huge = "h".repeat(300_000);
    append(&log, &format!("{huge}\nsmall\n"));
    let page = read(&log, Some("start"), 1000).unwrap();
    assert_eq!(texts(&page), [huge.as_str()]);
    assert!(page.more);
    let tail = read(&log, None, 1000).unwrap();
    assert_eq!(texts(&tail), ["small"]);
}

#[test]
fn a_line_is_its_text_without_the_line_ending() {
    let (_temp, log) = log();
    append(&log, "one\r\ntwo\n\nlast words \n");
    let page = read(&log, None, 10).unwrap();
    assert_eq!(texts(&page), ["one", "two", "", "last words "]);
}

// --- Which files are the log ---------------------------------------------------------------------

#[test]
fn only_the_active_file_and_its_dated_files_are_the_log_and_the_date_orders_them() {
    let (temp, log) = log();
    let dir = temp.path();
    // Dated files, written out of order: the date says which is older, not the order they were made.
    append(&dir.join(format!("{NAME}_2026-11-18_10-00-02.log")), "second\n");
    append(&dir.join(format!("{NAME}_2026-11-18_09-59-59.log")), "first\n");
    append(&log, "third\n");
    // What is not the log, though it looks like it.
    for other in [
        format!("{NAME}_2026-11-18_10-00-01.log.bak"),
        format!("{NAME}_notes.log"),
        format!("{NAME}.log.1"),
        format!("{NAME}-2026-11-18_10-00-01.log"),
        "other_2026-11-18_10-00-01.log".to_owned(),
        format!("{NAME}_2026-11-18_10-00-0x.log"),
    ] {
        append(&dir.join(other), "not the log\n");
    }
    let page = read(&log, None, 10).unwrap();
    assert_eq!(texts(&page), ["first", "second", "third"]);
}

#[test]
fn a_folder_that_is_not_there_is_an_empty_log() {
    let (temp, _) = log();
    let gone = temp.path().join("gone").join(format!("{NAME}.log"));
    let page = read(&gone, None, 10).unwrap();
    assert!(page.lines.is_empty());
    assert_eq!(page.cursor, "start");
}

// --- Holding on for a line -------------------------------------------------------------------------

const POLL: Duration = Duration::from_millis(20);

#[test]
fn a_read_that_waits_returns_at_its_limit_with_the_cursor_unchanged() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 2));
    let cursor = read(&log, None, 10).unwrap().cursor;
    let started = Instant::now();
    let page = hold(&log, Some(&cursor), 10, Duration::from_millis(400), POLL).unwrap();
    let waited = started.elapsed();
    assert!(page.lines.is_empty() && !page.more);
    assert_eq!(page.cursor, cursor);
    assert!(waited >= Duration::from_millis(350) && waited < Duration::from_secs(3), "{waited:?}");
    // Asking for no wait is asking for none.
    let started = Instant::now();
    let page = read_waiting(&log, Some(&cursor), None, Duration::ZERO).unwrap();
    assert!(page.lines.is_empty() && started.elapsed() < Duration::from_millis(300));
}

#[test]
fn a_new_line_wakes_a_read_that_waits_and_so_does_the_first_line_of_an_empty_log() {
    let (_temp, log) = log();
    let after = |text: &'static str| {
        let log = log.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            append(&log, text);
        })
    };
    let writer = after("first\n");
    let started = Instant::now();
    let page = hold(&log, None, 10, Duration::from_secs(10), POLL).unwrap();
    writer.join().unwrap();
    assert_eq!(texts(&page), ["first"]);
    assert!(started.elapsed() < Duration::from_secs(5));
    let writer = after("second\n");
    let started = Instant::now();
    let page = hold(&log, Some(&page.cursor), 10, Duration::from_secs(10), POLL).unwrap();
    writer.join().unwrap();
    assert_eq!(texts(&page), ["second"]);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_line_that_is_not_finished_does_not_wake_a_read_that_waits() {
    let (_temp, log) = log();
    append(&log, "whole\nhalf a li");
    let tail = read(&log, None, 10).unwrap();
    assert_eq!(texts(&tail), ["whole"]);
    let page = hold(&log, Some(&tail.cursor), 10, Duration::from_millis(400), POLL).unwrap();
    assert!(page.lines.is_empty());
    assert_eq!(page.cursor, tail.cursor);
}

#[test]
fn a_request_outside_the_bounds_is_invalid_whatever_it_waits_for() {
    let (_temp, log) = log();
    append(&log, &numbered(0, 3));
    for limit in [0, 1001, usize::MAX] {
        assert_eq!(read_waiting(&log, None, Some(limit), Duration::ZERO).err(), Some(Refused::Invalid));
    }
    assert_eq!(read_waiting(&log, Some("garbage"), None, Duration::from_secs(60)).err(), Some(Refused::Invalid));
    assert_eq!(read_waiting(&log, None, Some(1000), Duration::ZERO).unwrap().lines.len(), 3);
    assert_eq!(read_waiting(&log, None, None, Duration::ZERO).unwrap().lines.len(), 3);
}

// --- What a log file must be -----------------------------------------------------------------------

#[test]
fn a_log_the_plugin_made_is_read_though_it_is_not_private_and_what_is_not_a_file_is_not() {
    let (temp, log) = log();
    // As tauri-plugin-log makes it: the process's umask, no access narrowed. A file of this user's is all it must be.
    append(&log, &numbered(0, 2));
    assert_eq!(texts(&read(&log, None, 10).unwrap()), ["line 0", "line 1"]);
    // A dated name that is a folder, not a file, makes the log unreadable rather than quietly shorter.
    fs::create_dir(temp.path().join(format!("{NAME}_2026-11-18_09-00-00.log"))).unwrap();
    assert_eq!(read(&log, None, 10).err(), Some(Refused::Unreadable));
    assert_eq!(read_waiting(&log, None, None, Duration::ZERO).err(), Some(Refused::Unreadable));
}
