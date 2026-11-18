//! The page's problems in the app's log (`js/webview-log.js`), checked here: the page is not trusted, so this side, not the
//! script, is the allowlist (logging guideline section 8). The page sends a structured entry; only what is listed below is
//! written, as a line of ours with target `webview` that this side renders, and anything else is dropped and counted, its
//! content never logged.
//!
//! Allowed: the level (`warn`, `error`), the source (`console`, `uncaught`, `rejection`), at most [`MAX_ITEMS`] items, each a
//! string (as `[string]`, never its text), a number, a boolean, null or undefined, or an object's `name`, `code` and `key`
//! (each `[A-Za-z0-9._-]{1,64}`, else `invalid`) and first stack frame (file name of the same grammar, line, column).
//! No other field, no free text. At most [`MAX_PER_WINDOW`] entries per [`WINDOW`]; the rest are counted.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use tauri::{Runtime, State, Webview};

use crate::Callers;

const MAX_ITEMS: usize = 16;
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

/// `[A-Za-z0-9._-]{1,64}`, else `invalid`.
fn token(value: &str) -> &str {
    let ok = (1..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if ok {
        value
    } else {
        "invalid"
    }
}

fn frame(at: &Frame) -> String {
    let column = at.column.map(|c| format!(":{c}")).unwrap_or_default();
    format!("{}:{}{column}", token(&at.file), at.line)
}

fn item(item: &Item) -> String {
    match item {
        Item::String {} => "[string]".into(),
        Item::Number { value } => value.to_string(),
        Item::Bool { value } => value.to_string(),
        Item::Null {} => "null".into(),
        Item::Undefined {} => "undefined".into(),
        Item::Other {} => "[other]".into(),
        Item::Object { name, code, key, at } => {
            let mut parts = Vec::new();
            for (field, value) in [("name", name), ("code", code), ("key", key)] {
                if let Some(value) = value {
                    parts.push(format!("{field}={}", token(value)));
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
        tracing::warn!(target: "webview", dropped = outcome.lost, "the page's log entries were dropped (refused or too many)");
    }
    if let Some(Line { error, source, detail }) = outcome.line {
        if error {
            tracing::error!(target: "webview", source, detail = %detail, "the page reported a problem");
        } else {
            tracing::warn!(target: "webview", source, detail = %detail, "the page reported a problem");
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
