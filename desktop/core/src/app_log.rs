//! The app's own log read in pages: whole lines only, and a cursor that survives rotation.
//!
//! The log is `<name>.log` in the app's log folder, and the dated files tauri-plugin-log renames it to when it is full
//! (`<name>_<date>.log`, the newest `KeepSome` of them). A cursor is a generation (the hash of its first line, so a
//! rename does not move it and no path, inode or pid is ever shown) and an offset in it, always just after a newline.
//! A line's id is the cursor just before it.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use sidevoice_platform::fs::{Files, Fs};

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;
/// A page's soft size: lines are added while they fit, the first one whatever its size.
const PAGE_BYTES: usize = 256 * 1024;
const MAX_CURSOR: usize = 128;
/// The cursor of the beginning of the log: the answer for a log with no line yet.
const START: &str = "start";

#[derive(Serialize)]
pub struct Line {
    pub id: String,
    pub text: String,
}

#[derive(Serialize)]
pub struct Page {
    pub lines: Vec<Line>,
    pub cursor: String,
    pub more: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The cursor is not one this reader gives.
    Invalid,
    /// The cursor's generation is gone, or the file is shorter than the cursor says.
    Expired,
    Unreadable,
}

impl From<io::Error> for Refused {
    fn from(_: io::Error) -> Self {
        Self::Unreadable
    }
}

/// The most a read holds on for a line, whatever it asked.
const MAX_WAIT: Duration = Duration::from_secs(5);
/// How often a held read looks again.
const POLL: Duration = Duration::from_millis(250);

/// What the app log gives a read: the latest `limit` lines (default 200, at most 1000) when `after` is `None`, else
/// the lines written after the cursor `after`, oldest first, of the log whose newest file is `active`. With no line to
/// give it holds on for one for at most `wait`, and never more than 5 seconds; then it answers no line, with the
/// cursor unchanged. Blocks: run it where blocking is fine.
pub fn read_waiting(active: &Path, after: Option<&str>, limit: Option<usize>, wait: Duration) -> Result<Page, Refused> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(Refused::Invalid);
    }
    hold(active, after, limit, wait.min(MAX_WAIT), POLL)
}

fn hold(active: &Path, after: Option<&str>, limit: usize, wait: Duration, poll: Duration) -> Result<Page, Refused> {
    let deadline = Instant::now() + wait;
    loop {
        let page = read(active, after, limit)?;
        let left = deadline.saturating_duration_since(Instant::now());
        if !page.lines.is_empty() || left.is_zero() {
            return Ok(page);
        }
        std::thread::sleep(left.min(poll));
    }
}

fn read(active: &Path, after: Option<&str>, limit: usize) -> Result<Page, Refused> {
    pages(open(active)?, after, limit)
}

/// The files that exist, newest first: `active` is opened before the folder is listed, so a rotation in between shows
/// it again under its dated name (told apart by its first line) and hides nothing. A link is refused.
fn open(active: &Path) -> io::Result<Vec<File>> {
    let mut files = Vec::new();
    let mut take = |path: &Path| match Files::open_read(path) {
        Ok(file) => {
            files.push(file);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    };
    take(active)?;
    for path in dated(active)? {
        take(&path)?;
    }
    Ok(files)
}

/// The dated files beside `active`, newest first: `<stem>_<YYYY-MM-DD_HH-MM-SS>.log`, which sorts by date.
fn dated(active: &Path) -> io::Result<Vec<PathBuf>> {
    let (Some(dir), Some(stem)) = (active.parent(), active.file_stem().and_then(|stem| stem.to_str())) else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut dated = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        let date = name.to_str().and_then(|name| name.strip_prefix(stem)?.strip_prefix('_')?.strip_suffix(".log"));
        if let Some(date) = date.filter(|date| {
            date.len() == 19 && date.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
        }) {
            dated.push((date.to_owned(), dir.join(&name)));
        }
    }
    dated.sort();
    Ok(dated.into_iter().rev().map(|(_, path)| path).collect())
}

// From here on the reader is the one the core has for its own log (sidevoice-core, rust/server/logs/reader.rs): both
// are held to the same list of behaviours, so they change together.

/// [`read`] over `files`, newest first.
fn pages(files: Vec<File>, after: Option<&str>, limit: usize) -> Result<Page, Refused> {
    let cursor = after.map(Cursor::parse).transpose()?;
    let generations = generations(files)?;
    let (lines, last, more) = match cursor {
        None => {
            let (lines, last) = tail(&generations, limit)?;
            (lines, last, false)
        }
        Some(Cursor::Start) => forward(&generations, 0, 0, limit)?,
        Some(Cursor::At(id, offset)) => {
            let index = generations.iter().position(|generation| generation.id == id).ok_or(Refused::Expired)?;
            let generation = &generations[index];
            if offset > generation.end {
                return Err(Refused::Expired);
            }
            if offset > 0 && !generation.after_newline(offset)? {
                return Err(Refused::Invalid);
            }
            forward(&generations, index, offset, limit)?
        }
    };
    Ok(Page {
        lines,
        // A page with no line leaves the cursor where it was.
        cursor: last.unwrap_or_else(|| after.unwrap_or(START).to_owned()),
        more,
    })
}

enum Cursor {
    Start,
    At(u64, u64),
}

impl Cursor {
    fn parse(text: &str) -> Result<Self, Refused> {
        if text == START {
            return Ok(Self::Start);
        }
        let digits = |part: &str, hex: bool| {
            !part.is_empty()
                && part.bytes().all(|byte| if hex { byte.is_ascii_hexdigit() } else { byte.is_ascii_digit() })
        };
        match text.split_once('.') {
            Some((id, offset))
                if text.len() <= MAX_CURSOR && id.len() == 16 && digits(id, true) && digits(offset, false) =>
            {
                let id = u64::from_str_radix(id, 16).map_err(|_| Refused::Invalid)?;
                let offset = offset.parse().map_err(|_| Refused::Invalid)?;
                Ok(Self::At(id, offset))
            }
            _ => Err(Refused::Invalid),
        }
    }
}

fn at(id: u64, offset: u64) -> String {
    format!("{id:016x}.{offset}")
}

/// A generation as it was when opened: `end` is just after its last complete line.
struct Generation {
    file: File,
    id: u64,
    end: u64,
}

/// A line found, with the offset it ends at and its size in the file.
struct Found {
    line: Line,
    end: u64,
    size: usize,
}

impl Generation {
    /// `None` while it has no complete line: it has no identity yet.
    fn of(file: File) -> io::Result<Option<Self>> {
        let end = complete_end(&file)?;
        if end == 0 {
            return Ok(None);
        }
        let mut first = Vec::new();
        (&file).seek(SeekFrom::Start(0))?;
        BufReader::new((&file).take(end)).read_until(b'\n', &mut first)?;
        first.pop();
        let digest = Sha256::digest(&first);
        let id = u64::from_be_bytes(digest[..8].try_into().expect("eight bytes"));
        Ok(Some(Self { file, id, end }))
    }

    fn after_newline(&self, offset: u64) -> io::Result<bool> {
        let mut byte = [0u8];
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset - 1))?;
        file.read_exact(&mut byte)?;
        Ok(byte[0] == b'\n')
    }

    /// The first line start at or after `offset`, or the end.
    fn line_start_from(&self, offset: u64) -> io::Result<u64> {
        if offset == 0 {
            return Ok(0);
        }
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset - 1))?;
        let mut reader = BufReader::new(file.take(self.end.saturating_sub(offset - 1)));
        let mut skipped = Vec::new();
        Ok(offset - 1 + reader.read_until(b'\n', &mut skipped)? as u64)
    }

    /// The complete lines from `from` (a line start): at most `max_lines` and `max_bytes`, the first line however big
    /// when `first_free`.
    fn lines_from(&self, from: u64, max_lines: usize, max_bytes: usize, first_free: bool) -> io::Result<Vec<Found>> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(from))?;
        let mut reader = BufReader::new(file.take(self.end.saturating_sub(from)));
        let (mut lines, mut offset, mut bytes, mut raw) = (Vec::new(), from, 0usize, Vec::new());
        while lines.len() < max_lines {
            raw.clear();
            if reader.read_until(b'\n', &mut raw)? == 0
                || (bytes + raw.len() > max_bytes && !(first_free && lines.is_empty()))
            {
                break;
            }
            let text = raw.strip_suffix(b"\n").unwrap_or(&raw);
            let text = text.strip_suffix(b"\r").unwrap_or(text);
            lines.push(Found {
                line: Line { id: at(self.id, offset), text: String::from_utf8_lossy(text).into_owned() },
                end: offset + raw.len() as u64,
                size: raw.len(),
            });
            offset += raw.len() as u64;
            bytes += raw.len();
        }
        Ok(lines)
    }
}

/// The offset just after the last newline of the file: a last line still being written is not part of it yet.
fn complete_end(mut file: &File) -> io::Result<u64> {
    let mut end = file.metadata()?.len();
    let mut chunk = [0u8; 4096];
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        let part = &mut chunk[..(end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(part)?;
        if let Some(newline) = part.iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + newline as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

/// The generations that have a line, oldest first, of `files` newest first.
fn generations(files: Vec<File>) -> io::Result<Vec<Generation>> {
    let mut found = Vec::new();
    for file in files {
        found.extend(Generation::of(file)?);
    }
    found.reverse();
    found.dedup_by(|later, kept| {
        let same = later.id == kept.id;
        if same {
            kept.end = kept.end.max(later.end);
        }
        same
    });
    Ok(found)
}

/// The lines after `offset` in generation `first` and then in each newer one: up to `limit` lines and a page's size.
/// With the cursor after the last of them, and whether more is already there.
fn forward(
    generations: &[Generation],
    first: usize,
    offset: u64,
    limit: usize,
) -> io::Result<(Vec<Line>, Option<String>, bool)> {
    let (mut lines, mut cursor, mut bytes) = (Vec::new(), None, 0);
    for (index, generation) in generations.iter().enumerate().skip(first) {
        let from = if index == first { offset } else { 0 };
        let read =
            generation.lines_from(from, limit - lines.len(), PAGE_BYTES.saturating_sub(bytes), lines.is_empty())?;
        let reached = read.last().map_or(from, |last| last.end);
        if let Some(last) = read.last() {
            cursor = Some(at(generation.id, last.end));
        }
        bytes += read.iter().map(|line| line.size).sum::<usize>();
        lines.extend(read.into_iter().map(|read| read.line));
        if lines.len() == limit || reached < generation.end {
            let more = reached < generation.end || index + 1 < generations.len();
            return Ok((lines, cursor, more));
        }
    }
    Ok((lines, cursor, false))
}

/// The latest lines, at most `limit` and a page's size, oldest first, with the cursor after the last.
fn tail(generations: &[Generation], limit: usize) -> io::Result<(Vec<Line>, Option<String>)> {
    let (mut newest_first, mut cursor, mut bytes) = (Vec::new(), None, 0);
    'older: for generation in generations.iter().rev() {
        if newest_first.len() == limit {
            break;
        }
        // A window of the end of the generation; wider only when it holds no whole line.
        let mut window = PAGE_BYTES.saturating_sub(bytes).max(1) as u64;
        let (from, read) = loop {
            let from = generation.line_start_from(generation.end.saturating_sub(window))?;
            let read = generation.lines_from(from, usize::MAX, usize::MAX, true)?;
            if from == 0 || !read.is_empty() {
                break (from, read);
            }
            window *= 2;
        };
        for line in read.into_iter().rev() {
            if newest_first.len() == limit || (!newest_first.is_empty() && bytes + line.size > PAGE_BYTES) {
                break 'older;
            }
            if newest_first.is_empty() {
                cursor = Some(at(generation.id, line.end));
            }
            bytes += line.size;
            newest_first.push(line.line);
        }
        // What is older than the window is out of the page's size.
        if from > 0 {
            break;
        }
    }
    newest_first.reverse();
    Ok((newest_first, cursor))
}

#[cfg(test)]
mod tests;
