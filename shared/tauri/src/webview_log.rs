//! The page's problems in the app's log (`js/webview-log.js`), checked here: the page is not trusted, so this side, not the
//! script, is the allowlist (logging guideline section 8). The page sends a structured entry; only what is listed below is
//! written, as a line of ours with target `webview` that this side renders, and anything else is dropped and counted, its
//! content never logged.
//!
//! Allowed: the level (`warn`, `error`), the source (`console`, `uncaught`, `rejection`), at most [`MAX_ITEMS`] items, each a
//! string (as `[string]`, never its text), a number, a boolean, null or undefined, or an object's `name`, `code` and `key`
//! (`name` as `...Error` or `...Exception`, `code` and `key` as a dotted code such as `voice.failure.model`, else `invalid`) and first stack frame (file name `[A-Za-z0-9._-]{1,64}`, line, column).
//! No other field, no free text. At most [`MAX_PER_WINDOW`] entries per [`WINDOW`]; the rest are counted.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use tauri::{Runtime, State, Webview};

use crate::Callers;

/// The target of every line this module writes. Only our own targets may reach the crash ring buffer, the mobile system log
/// and the handled-error events, and each of those must exclude or include this one by name: a line of the page's, even a
/// checked one, must not be written under any other (a test keeps every call here to it).
const TARGET: &str = "webview";
const MAX_ITEMS: usize = 16;
/// The most a rendered detail holds, in bytes; the rest is cut and marked.
const MAX_DETAIL: usize = 1024;
const MAX_PER_WINDOW: u32 = 50;
const WINDOW: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Warn,
    Error,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Source {
    Console,
    Uncaught,
    Rejection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    file: String,
    line: u32,
    column: Option<u32>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Item {
    String {},
    Number { value: f64 },
    Bool { value: bool },
    Null {},
    Undefined {},
    Other {},
    Object { name: Option<String>, code: Option<String>, key: Option<String>, at: Option<Frame> },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    level: Level,
    source: Source,
    items: Vec<Item>,
    at: Option<Frame>,
}

/// A line to write: the level (`true` for error), the source and the rendered detail.
#[derive(Debug, PartialEq)]
pub(crate) struct Line {
    pub error: bool,
    pub source: &'static str,
    pub detail: String,
}

/// `[A-Za-z0-9._-]{1,64}`, else `invalid`: a file name.
fn token(value: &str) -> &str {
    let ok = (1..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if ok {
        value
    } else {
        "invalid"
    }
}

/// `[A-Za-z]{1,40}(Error|Exception)`, else `invalid`.
fn name(value: &str) -> &str {
    let stem = value.strip_suffix("Error").or_else(|| value.strip_suffix("Exception"));
    let ok = stem.is_some_and(|stem| (1..=40).contains(&stem.len()) && stem.bytes().all(|b| b.is_ascii_alphabetic()));
    if ok {
        value
    } else {
        "invalid"
    }
}

/// A dotted code, as the page's script allows (else `invalid`): `voice.failure.model` (a lowercase word of up to 32
/// characters, then two to four parts), or one of ours under `voice`, `core`, `engine`, `app` or `call` (one to four
/// parts after it). Parts are `[a-z0-9_]{1,32}`.
fn code(value: &str) -> &str {
    let part = |p: &str| {
        (1..=32).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    let mut parts = value.split('.');
    let first = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    let dotted = first.starts_with(|c: char| c.is_ascii_lowercase()) && part(first) && (2..=4).contains(&rest.len());
    let ours = ["voice", "core", "engine", "app", "call"].contains(&first) && (1..=4).contains(&rest.len());
    if (dotted || ours) && rest.iter().all(|p| part(p)) {
        value
    } else {
        "invalid"
    }
}

/// A number as compactly as it reads: an integer below 1e15 as it is, anything else in exponent form.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        value.to_string()
    } else {
        format!("{value:e}")
    }
}

fn frame(at: &Frame) -> String {
    let column = at.column.map(|c| format!(":{c}")).unwrap_or_default();
    format!("{}:{}{column}", token(&at.file), at.line)
}

fn item(item: &Item) -> String {
    match item {
        Item::String {} => "[string]".into(),
        Item::Number { value } => number(*value),
        Item::Bool { value } => value.to_string(),
        Item::Null {} => "null".into(),
        Item::Undefined {} => "undefined".into(),
        Item::Other {} => "[other]".into(),
        Item::Object { name: n, code: c, key: k, at } => {
            let mut parts = Vec::new();
            for (field, value, valid) in [("name", n, name as fn(&str) -> &str), ("code", c, code), ("key", k, code)] {
                if let Some(value) = value {
                    parts.push(format!("{field}={}", valid(value)));
                }
            }
            if let Some(at) = at {
                parts.push(format!("at={}", frame(at)));
            }
            if parts.is_empty() {
                "[object]".into()
            } else {
                format!("{{{}}}", parts.join(" "))
            }
        }
    }
}

/// The line an entry makes, or `None` when it is not exactly what is allowed.
fn render(entry: Value) -> Option<Line> {
    let entry: Entry = serde_json::from_value(entry).ok()?;
    if entry.items.len() > MAX_ITEMS {
        return None;
    }
    let mut detail = entry.items.iter().map(item).collect::<Vec<_>>().join(" ");
    if let Some(at) = &entry.at {
        detail.push_str(&format!(" at {}", frame(at)));
    }
    if detail.len() > MAX_DETAIL {
        // Every part is ASCII, so any index is a character boundary.
        detail.truncate(MAX_DETAIL);
        detail.push_str("[truncated]");
    }
    let source = match entry.source {
        Source::Console => "console",
        Source::Uncaught => "uncaught",
        Source::Rejection => "rejection",
    };
    Some(Line { error: matches!(entry.level, Level::Error), source, detail })
}

/// The rate of entries, and how many were dropped (refused or over the rate) since the last report.
#[derive(Default)]
pub struct Limiter(Mutex<(Option<Instant>, u32, u32)>);

/// What one entry makes: the dropped count of the window that just ended, if any, and the line to write.
#[derive(Debug, PartialEq)]
pub(crate) struct Outcome {
    pub lost: u32,
    pub line: Option<Line>,
}

impl Limiter {
    pub(crate) fn handle(&self, now: Instant, entry: Value) -> Outcome {
        let mut state = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (start, sent, dropped) = &mut *state;
        let mut lost = 0;
        if start.is_none_or(|start| now.duration_since(start) > WINDOW) {
            lost = std::mem::take(dropped);
            *start = Some(now);
            *sent = 0;
        }
        let line = if *sent >= MAX_PER_WINDOW { None } else { render(entry) };
        if line.is_some() {
            *sent += 1;
        } else {
            *dropped += 1;
        }
        Outcome { lost, line }
    }
}

/// One of the page's problems for the app's log (the log script). Same caller rule as the voice commands. What is not
/// allowed is dropped without an answer that tells the page why.
#[tauri::command]
pub fn webview_log<R: Runtime>(
    webview: Webview<R>,
    callers: State<'_, Callers<R>>,
    limiter: State<'_, Limiter>,
    entry: Value,
) -> Result<(), String> {
    callers.check(&webview)?;
    let outcome = limiter.handle(Instant::now(), entry);
    if outcome.lost > 0 {
        tracing::warn!(target: TARGET, dropped = outcome.lost, "the page's log entries were dropped (refused or too many)");
    }
    if let Some(Line { error, source, detail }) = outcome.line {
        if error {
            tracing::error!(target: TARGET, source, detail = %detail, "the page reported a problem");
        } else {
            tracing::warn!(target: TARGET, source, detail = %detail, "the page reported a problem");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn line(entry: Value) -> Option<String> {
        render(entry).map(|l| format!("{} {} {}", if l.error { "error" } else { "warn" }, l.source, l.detail))
    }

    #[test]
    fn an_allowed_entry_is_written_as_rendered_here() {
        let entry = json!({
            "level": "error", "source": "console",
            "items": [
                { "kind": "string" }, { "kind": "number", "value": 3 }, { "kind": "bool", "value": true },
                { "kind": "object", "code": "voice.failure.model", "at": { "file": "app.js", "line": 4, "column": 9 } }
            ]
        });
        assert_eq!(line(entry).unwrap(), "error console [string] 3 true {code=voice.failure.model at=app.js:4:9}");
    }

    #[test]
    fn what_is_not_listed_is_dropped() {
        for entry in [
            json!({ "level": "info", "source": "console", "items": [] }),
            json!({ "level": "warn", "source": "page", "items": [] }),
            json!({ "level": "warn", "source": "console", "items": [], "message": "text" }),
            json!({ "level": "warn", "source": "console", "items": [{ "kind": "string", "value": "text" }] }),
            json!({ "level": "warn", "source": "console", "items": [{ "kind": "object", "message": "text" }] }),
            json!({ "level": "warn", "source": "console", "items": [{ "kind": "number", "value": "text" }] }),
            json!({ "level": "warn", "source": "console", "items": vec![json!({ "kind": "null" }); MAX_ITEMS + 1] }),
            json!("free text"),
            json!(null),
        ] {
            assert_eq!(line(entry.clone()), None, "{entry}");
        }
    }

    #[test]
    fn a_name_code_key_or_file_outside_the_grammar_is_invalid_and_never_written() {
        let hostile = "canary words 7f3a\nforged line";
        let long = "a".repeat(65);
        let entry = json!({
            "level": "warn", "source": "uncaught", "items": [
                { "kind": "object", "name": hostile, "code": long, "key": "" }
            ],
            "at": { "file": "/home/canary-user-7f3a/x.js", "line": 1 }
        });
        let written = line(entry).unwrap();
        assert_eq!(written, "warn uncaught {name=invalid code=invalid key=invalid} at invalid:1");
        assert!(!written.contains("canary"));
    }

    #[test]
    fn a_code_key_or_name_that_is_not_the_pages_grammar_is_invalid() {
        let entry = |field: &str, value: &str| json!({ "level": "warn", "source": "console", "items": [{ "kind": "object", field: value }] });
        for (field, value) in [
            ("code", "K7Q2-9XPD"),
            ("key", "KeyA"),
            ("code", "a.b"),
            ("name", "Error"),
            ("name", "NotAnError "),
            ("name", "T1Error"),
        ] {
            assert_eq!(
                line(entry(field, value)).unwrap(),
                format!("warn console {{{field}=invalid}}"),
                "{field} {value}"
            );
        }
        assert_eq!(line(entry("code", "core.refused")).unwrap(), "warn console {code=core.refused}");
        assert_eq!(line(entry("name", "AbortError")).unwrap(), "warn console {name=AbortError}");
    }

    #[test]
    fn a_detail_is_capped_and_numbers_are_compact() {
        let items = vec![json!({ "kind": "number", "value": 1e308 }); MAX_ITEMS];
        let written = render(json!({ "level": "warn", "source": "console", "items": items })).unwrap().detail;
        assert!(written.starts_with("1e308 1e308"), "{written}");
        assert!(written.len() < MAX_DETAIL);
        assert_eq!(number(3.0), "3");
        assert_eq!(number(0.1), "1e-1");
        let many = vec![
            json!({ "kind": "object", "name": "a".repeat(40) + "Exception", "code": "voice.failure.model" });
            MAX_ITEMS
        ];
        let cut = render(json!({ "level": "warn", "source": "console", "items": many })).unwrap().detail;
        assert_eq!(cut.len(), MAX_DETAIL + "[truncated]".len());
        assert!(cut.ends_with("[truncated]"));
    }

    #[test]
    fn every_line_this_module_writes_has_the_webview_target() {
        let source = include_str!("webview_log.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        let calls = code.matches("tracing::").count();
        assert!(calls >= 3);
        assert_eq!(code.matches("target: TARGET").count(), calls, "a tracing call without the webview target");
        assert_eq!(TARGET, "webview");
    }

    #[test]
    fn a_flood_is_dropped_and_counted_then_reported_once() {
        let limiter = Limiter::default();
        let ok = || json!({ "level": "warn", "source": "console", "items": [{ "kind": "null" }] });
        let start = Instant::now();
        for _ in 0..MAX_PER_WINDOW {
            assert!(limiter.handle(start, ok()).line.is_some());
        }
        assert_eq!(limiter.handle(start, ok()), Outcome { lost: 0, line: None });
        assert_eq!(limiter.handle(start, json!("refused")), Outcome { lost: 0, line: None });
        let later = limiter.handle(start + WINDOW + Duration::from_secs(1), ok());
        assert_eq!(later.lost, 2);
        assert!(later.line.is_some());
        assert_eq!(limiter.handle(start + WINDOW + Duration::from_secs(1), ok()).lost, 0);
    }
}
