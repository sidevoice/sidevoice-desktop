//! The app's logging, shared by the desktop and the phone: `SIDEVOICE_LOG`, the one subscriber and the panic hook
//! (docs: the Sidevoice logging guideline, sidevoice/.github-private `docs/logging.md`). The process installs it once; libraries only
//! emit.

use std::io::{IsTerminal, Write};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// The variable that sets the log's level and filters, in `EnvFilter` syntax (default `info`). No other variable is read.
pub const VARIABLE: &str = "SIDEVOICE_LOG";

const LEVELS: [&str; 6] = ["error", "warn", "info", "debug", "trace", "off"];

/// The filter `value` asks for, and whether it was invalid (then the default is used and the caller says so with one
/// `warn`). A bare level applies to `own` targets only; every other target stays at `warn` unless `value` names it.
/// A value with no bare level (only targets) gets the default `info` for `own` targets.
pub fn filter(value: Option<&str>, own: &[&str]) -> (EnvFilter, bool) {
    let default = expand("info", own);
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return (EnvFilter::new(default), false);
    };
    let mut bare = false;
    let mut directives: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .map(|directive| {
            if LEVELS.contains(&directive.to_ascii_lowercase().as_str()) {
                bare = true;
                expand(&directive.to_ascii_lowercase(), own)
            } else {
                directive.to_owned()
            }
        })
        .collect();
    if !bare {
        directives.insert(0, default.clone());
    }
    match EnvFilter::try_new(directives.join(",")) {
        Ok(filter) => (filter, false),
        Err(_) => (EnvFilter::new(default), true),
    }
}

/// `warn` for everything, `level` for each of `own` (`off` is off for everything).
fn expand(level: &str, own: &[&str]) -> String {
    if level == "off" {
        return "off".to_owned();
    }
    std::iter::once("warn".to_owned())
        .chain(own.iter().map(|target| format!("{target}={level}")))
        .collect::<Vec<_>>()
        .join(",")
}

/// A `MakeWriter` that hands each formatted event, one whole line, to `sink` (the app's rotating file).
pub struct Lines<F>(pub F);

pub struct Line<'a, F: Fn(&str)> {
    sink: &'a F,
    buffer: Vec<u8>,
}

impl<'a, F: Fn(&str) + 'a> MakeWriter<'a> for Lines<F> {
    type Writer = Line<'a, F>;
    fn make_writer(&'a self) -> Self::Writer {
        Line { sink: &self.0, buffer: Vec::new() }
    }
}

impl<F: Fn(&str)> Write for Line<'_, F> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<F: Fn(&str)> Drop for Line<'_, F> {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.buffer);
        let text = text.trim_end_matches('\n');
        if !text.is_empty() {
            (self.sink)(text);
        }
    }
}

/// Installs the process's subscriber: `SIDEVOICE_LOG` as the filter, one human `key=value` layer into `file`, and one into
/// stderr when stderr is a terminal and `SIDEVOICE_DEBUG` is not on (that channel prints its own lines there). It also
/// routes the `log` crate's records (tao, wry, tauri) into it. A second call does nothing.
pub fn init<W>(own: &[&str], file: W)
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    let (filter, invalid) = filter(std::env::var(VARIABLE).ok().as_deref(), own);
    let file = tracing_subscriber::fmt::layer().with_ansi(false).with_writer(file).log_internal_errors(false);
    let stderr = (std::io::stderr().is_terminal() && !crate::debug::enabled())
        .then(|| tracing_subscriber::fmt::layer().with_ansi(true).with_writer(std::io::stderr));
    let installed = tracing_subscriber::registry().with(file.and_then(stderr).with_filter(filter)).try_init().is_ok();
    if installed {
        install_panic_hook();
        if invalid {
            tracing::warn!(variable = VARIABLE, "invalid log filter, using the default");
        }
    }
}

/// A panic goes to the log (`target: "panic"`), then to the previous hook (stderr as before).
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message =
            payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        let thread = std::thread::current();
        tracing::error!(
            target: "panic",
            location = %info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default(),
            thread = thread.name().unwrap_or("unnamed"),
            panic = message.unwrap_or("non-string payload"),
            "panic"
        );
        previous(info);
    }));
}

/// An id a page or client named, as a log field: as it is when it is 1 to 64 of `[A-Za-z0-9._:-]`, else `invalid`. The sender
/// chooses the bytes, so a hostile id could forge log lines.
pub fn client_id(id: &str) -> &str {
    let plain = (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b));
    if plain {
        id
    } else {
        "invalid"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &[&str] = &["sidevoice_shared", "panic"];

    fn text(value: Option<&str>) -> String {
        let (filter, invalid) = filter(value, OWN);
        assert!(!invalid, "{value:?}");
        let mut directives: Vec<String> = filter.to_string().split(',').map(str::to_owned).collect();
        directives.sort();
        directives.join(",")
    }

    #[test]
    fn unset_and_empty_use_info_for_our_targets_and_warn_for_the_rest() {
        for value in [None, Some(""), Some("  ")] {
            assert_eq!(text(value), "panic=info,sidevoice_shared=info,warn");
        }
    }

    #[test]
    fn a_bare_level_applies_to_our_targets_only() {
        assert_eq!(text(Some(" DEBUG ")), "panic=debug,sidevoice_shared=debug,warn");
        assert_eq!(text(Some("info,reqwest=debug")), "panic=info,reqwest=debug,sidevoice_shared=info,warn");
        assert_eq!(text(Some("off")), "off");
    }

    #[test]
    fn directives_naming_a_target_keep_the_default_around_them() {
        assert_eq!(
            text(Some("sidevoice_shared::debug=trace")),
            "panic=info,sidevoice_shared::debug=trace,sidevoice_shared=info,warn"
        );
    }

    #[test]
    fn an_invalid_value_falls_back_to_the_default_and_says_so() {
        let (filter, invalid) = filter(Some("info,x=loud"), OWN);
        assert!(invalid);
        assert_eq!(filter.to_string(), "sidevoice_shared=info,panic=info,warn");
    }

    #[test]
    fn a_line_reaches_the_sink_whole() {
        use std::sync::Mutex;
        let seen = Mutex::new(Vec::new());
        let lines = Lines(|line: &str| seen.lock().unwrap().push(line.to_owned()));
        let mut writer = lines.make_writer();
        writer.write_all(b"one ").unwrap();
        writer.write_all(b"line\n").unwrap();
        drop(writer);
        assert_eq!(*seen.lock().unwrap(), ["one line"]);
    }

    #[test]
    fn a_client_id_is_plain_or_invalid() {
        assert_eq!(client_id("turn-1.a:b_c"), "turn-1.a:b_c");
        for hostile in ["", "a b", "x\nINFO forged", &"a".repeat(65), "é"] {
            assert_eq!(client_id(hostile), "invalid");
        }
    }
}
