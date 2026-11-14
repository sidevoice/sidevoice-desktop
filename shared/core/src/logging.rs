//! The app's logging, shared by the desktop and the phone: `SIDEVOICE_LOG`, the one subscriber and the panic hook
//! (docs: the Sidevoice logging guideline, sidevoice/.github-private `docs/logging.md`). The process installs it once; libraries only
//! emit.

use std::cell::Cell;
use std::io::{IsTerminal, Write};
use std::sync::{Mutex, PoisonError};
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

/// For each of `own` the `level`; for everything else the quieter of it and `warn` (`off` is off for everything).
fn expand(level: &str, own: &[&str]) -> String {
    if level == "off" {
        return "off".to_owned();
    }
    let others = if level == "error" { "error" } else { "warn" };
    std::iter::once(others.to_owned())
        .chain(own.iter().map(|target| format!("{target}={level}")))
        .collect::<Vec<_>>()
        .join(",")
}

/// The home directory the process resolved, in every spelling it may print in (plain, `Debug`-escaped, with forward slashes),
/// matched ignoring ASCII case: the writer's backstop for third-party lines and panic text, which shows it as `~`. The types
/// that keep paths out of fields are the mechanism; this is defence in depth.
#[derive(Clone, Default)]
pub struct Home(Vec<String>);

impl Home {
    pub fn new(home: Option<&std::path::Path>) -> Home {
        Home::of(&home.map(|home| home.to_string_lossy().into_owned()).unwrap_or_default())
    }

    fn of(plain: &str) -> Home {
        let plain = plain.trim_end_matches(['/', '\\']).to_owned();
        let mut spellings = vec![plain.clone(), plain.replace('\\', "\\\\"), plain.replace('\\', "/")];
        spellings.retain(|spelling| spelling.len() > 1);
        spellings.sort_by_key(|spelling| std::cmp::Reverse(spelling.len()));
        spellings.dedup();
        Home(spellings)
    }

    pub fn redact(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        'scan: while let Some(next) = rest.chars().next() {
            for spelling in &self.0 {
                if rest.len() >= spelling.len()
                    && rest.is_char_boundary(spelling.len())
                    && rest[..spelling.len()].eq_ignore_ascii_case(spelling)
                {
                    out.push('~');
                    rest = &rest[spelling.len()..];
                    continue 'scan;
                }
            }
            out.push(next);
            rest = &rest[next.len_utf8()..];
        }
        out
    }
}

/// A `MakeWriter` that hands each formatted event, one whole line with the home directory shown as `~`, to `sink`.
pub struct Lines<F> {
    sink: F,
    home: Home,
}

pub struct Line<'a, F: Fn(&str)> {
    lines: &'a Lines<F>,
    buffer: Vec<u8>,
}

impl<'a, F: Fn(&str) + 'a> MakeWriter<'a> for Lines<F> {
    type Writer = Line<'a, F>;
    fn make_writer(&'a self) -> Self::Writer {
        Line { lines: self, buffer: Vec::new() }
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

/// One line at a time, whichever writer and thread: a writer waits for the previous line. Inside the panic hook (the thread
/// that panicked may hold the lock) it only tries, and writes anyway when it cannot.
static WRITING: Mutex<()> = Mutex::new(());

/// Whether this thread is inside the panic hook: a writer of its own must only try its locks then.
pub fn in_panic_hook() -> bool {
    IN_HOOK.with(Cell::get)
}

/// The home the writers redact, for the one line the panic hook writes itself.
static HOME: std::sync::OnceLock<Home> = std::sync::OnceLock::new();

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
}

impl<F: Fn(&str)> Drop for Line<'_, F> {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.buffer);
        let text = text.trim_end_matches('\n');
        if text.is_empty() {
            return;
        }
        let text = self.lines.home.redact(text);
        let _held = if IN_HOOK.with(Cell::get) {
            WRITING.try_lock().ok()
        } else {
            Some(WRITING.lock().unwrap_or_else(PoisonError::into_inner))
        };
        (self.lines.sink)(&text);
    }
}

/// What a process adds to [`init`]'s file layer: a layer of its own (the phone's system log), and which of the other parts it
/// wants.
pub struct Extras<L> {
    /// A layer of the process's own, under the same filter.
    pub layer: Option<L>,
    /// Whether stderr gets a layer when it is a terminal (never on the phone, where stderr is the system's public log).
    pub stderr: bool,
    /// Whether the panic hook is installed here (the phone's has its own, which never logs).
    pub panic_hook: bool,
}

/// Installs the process's subscriber: `SIDEVOICE_LOG` as the filter, and one human `key=value` layer into `sink` (the file) and
/// one into stderr when stderr is a terminal and `SIDEVOICE_DEBUG` is not on (that channel prints its own lines there). Both
/// show `home` as `~`. It also routes the `log` crate's records (tao, wry, tauri) into it, and a panic goes to it through a
/// hook. A second call does nothing.
pub fn init<F>(own: &[&str], home: Option<&std::path::Path>, sink: F)
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let extras: Extras<tracing_subscriber::layer::Identity> = Extras { layer: None, stderr: true, panic_hook: true };
    init_with(own, home, sink, extras);
}

/// [`init`] with `extras`.
pub fn init_with<F, L>(own: &[&str], home: Option<&std::path::Path>, sink: F, extras: Extras<L>)
where
    F: Fn(&str) + Send + Sync + 'static,
    L: Layer<tracing_subscriber::Registry> + Send + Sync + 'static,
{
    let (filter, invalid) = filter(std::env::var(VARIABLE).ok().as_deref(), own);
    let home = Home::new(home);
    let _ = HOME.set(home.clone());
    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(Lines { sink, home: home.clone() })
        .log_internal_errors(false);
    let stderr = (extras.stderr && std::io::stderr().is_terminal() && !crate::debug::enabled()).then(|| {
        let to_stderr = |line: &str| {
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        };
        tracing_subscriber::fmt::layer()
            .with_ansi(true)
            .with_writer(Lines { sink: to_stderr, home })
            .log_internal_errors(false)
    });
    let installed = tracing_subscriber::registry()
        .with(file.and_then(stderr).and_then(extras.layer).with_filter(filter))
        .try_init()
        .is_ok();
    if installed {
        if extras.panic_hook {
            install_panic_hook();
        }
        if invalid {
            tracing::warn!(variable = VARIABLE, "invalid log filter, using the default");
        }
    }
}

/// A panic goes to the log once, through the subscriber (`target: "panic"`: its place and thread, never its message, which `unwrap`
/// and `assert` fill with `Debug` of values), and its place to stderr. The
/// default hook is not chained: it would write the raw message to stderr, past the writer's backstop.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        IN_HOOK.with(|flag| flag.set(true));
        let thread = std::thread::current();
        tracing::error!(
            target: "panic",
            location = %info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default(),
            thread = thread.name().unwrap_or("unnamed"),
            "panic"
        );
        IN_HOOK.with(|flag| flag.set(false));
        // However the log is set up, a panic must not vanish: the place, never the message, on stderr as well.
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let location = HOME.get().map_or(location.clone(), |home| home.redact(&location));
        let _ = writeln!(std::io::stderr().lock(), "sidevoice: panic at {location}");
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
        assert_eq!(text(Some("error")), "error,panic=error,sidevoice_shared=error");
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
        let lines = Lines {
            sink: |line: &str| seen.lock().unwrap().push(line.to_owned()),
            home: Home::new(Some(std::path::Path::new("/home/ana"))),
        };
        let mut writer = lines.make_writer();
        writer.write_all(b"one /HOME/ana/x ").unwrap();
        writer.write_all(b"line\n").unwrap();
        drop(writer);
        assert_eq!(*seen.lock().unwrap(), ["one ~/x line"]);
    }

    #[test]
    fn a_client_id_is_plain_or_invalid() {
        assert_eq!(client_id("turn-1.a:b_c"), "turn-1.a:b_c");
        for hostile in ["", "a b", "x\nINFO forged", &"a".repeat(65), "é"] {
            assert_eq!(client_id(hostile), "invalid");
        }
    }

    #[test]
    fn the_home_is_redacted_in_every_spelling() {
        let home = Home::of("C:\\Users\\Ana");
        assert_eq!(home.redact("a C:\\Users\\Ana\\x b C:\\\\Users\\\\Ana c c:/users/ana/y"), "a ~\\x b ~ c ~/y");
        assert_eq!(Home::of("").redact("/x"), "/x");
        assert_eq!(Home::of("/").redact("/x"), "/x");
    }
}
