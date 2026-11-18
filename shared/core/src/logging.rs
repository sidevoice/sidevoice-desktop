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

/// The panic record: where, on which thread, which version and, for a literal, the message (guideline section 9).
pub const PANIC_TARGET: &str = "panic";
/// The backtrace of a panic, a second event for the app file only: never stderr, never a system log. It shares no prefix
/// with another of our targets (filters match targets by prefix), so no directive reaches it by accident.
pub const BACKTRACE_TARGET: &str = "crash_backtrace";
/// The page's console lines, checked and rendered by the shared plugin's `webview_log` (the one place that names it). Our own
/// target, but the page's words: the crash report's ring of log lines leaves it out (`crash::tail`), as the phone's system log does.
pub const WEBVIEW_TARGET: &str = "webview";
/// What a panic's message is replaced by when the payload is not a literal: a formatted message carries values.
pub const FORMATTED_MARKER: &str = "<formatted panic message omitted>";
const NON_TEXT_MARKER: &str = "<non-text panic payload omitted>";

/// Whether `target` is the backtrace event's, by the same prefix rule as the filter directive `crash_backtrace=...`.
pub fn is_backtrace_target(target: &str) -> bool {
    target.strip_prefix(BACKTRACE_TARGET).is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

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
    /// Whether the backtrace event is dropped here: true for every writer but the app file's.
    skip_backtrace: bool,
}

pub struct Line<'a, F: Fn(&str)> {
    lines: &'a Lines<F>,
    buffer: Vec<u8>,
    skipped: bool,
}

impl<'a, F: Fn(&str) + 'a> MakeWriter<'a> for Lines<F> {
    type Writer = Line<'a, F>;
    fn make_writer(&'a self) -> Self::Writer {
        Line { lines: self, buffer: Vec::new(), skipped: false }
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        Line { lines: self, buffer: Vec::new(), skipped: self.skip_backtrace && is_backtrace_target(meta.target()) }
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
        if text.is_empty() || self.skipped {
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
pub struct Extras<'a, L> {
    /// A layer of the process's own (the phone's system log). It is not under `SIDEVOICE_LOG`: it takes `warn` and `error` of
    /// `system_targets` only, never a backtrace (`system_log::filter`).
    pub layer: Option<L>,
    /// The targets `layer` takes: a subset of `own`, leaving out a target whose lines may carry what the page says.
    pub system_targets: &'a [&'a str],
    /// Whether stderr gets a layer when it is a terminal (never on the phone: its stderr is not ours to fill).
    pub stderr: bool,
    /// Whether the shared panic hook is installed.
    pub panic_hook: bool,
}

/// Installs the process's subscriber: `SIDEVOICE_LOG` as the filter, and one human `key=value` layer into `sink` (the file) and
/// one into stderr when stderr is a terminal and `SIDEVOICE_DEBUG` is not on (that channel prints its own lines there). Both
/// show `home` as `~`. It also routes the `log` crate's records (tao, wry, tauri) into it, and a panic goes to it through a
/// hook. A second call does nothing. Only the file layer gets a panic's backtrace.
pub fn init<F>(own: &[&str], home: Option<&std::path::Path>, sink: F)
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let extras: Extras<tracing_subscriber::layer::Identity> =
        Extras { layer: None, system_targets: &[], stderr: true, panic_hook: true };
    init_with(own, home, sink, extras);
}

/// [`init`] with `extras`.
pub fn init_with<F, L>(own: &[&str], home: Option<&std::path::Path>, sink: F, extras: Extras<'_, L>)
where
    F: Fn(&str) + Send + Sync + 'static,
    L: Layer<tracing_subscriber::Registry> + Send + Sync + 'static,
{
    let value = std::env::var(VARIABLE).ok();
    let (filter, invalid) = filter(value.as_deref(), own);
    let home = Home::new(home);
    let _ = HOME.set(home.clone());
    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(Lines { sink, home: home.clone(), skip_backtrace: false })
        .log_internal_errors(false);
    let stderr = (extras.stderr && std::io::stderr().is_terminal() && !crate::debug::enabled()).then(|| {
        let to_stderr = |line: &str| {
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        };
        tracing_subscriber::fmt::layer()
            .with_ansi(true)
            .with_writer(Lines { sink: to_stderr, home, skip_backtrace: true })
            .log_internal_errors(false)
    });
    STDERR_LAYER.store(stderr.is_some(), std::sync::atomic::Ordering::Relaxed);
    // The process's own layer (the phone's system log) has a filter of its own, whatever `SIDEVOICE_LOG` says.
    let extra = extras.layer.map(|layer| layer.with_filter(crate::system_log::filter(extras.system_targets)));
    // The last warnings and errors of our own targets, in memory, for the record of a crash (`crash::tail`): a layer of its own,
    // whatever `SIDEVOICE_LOG` says.
    let tail = crate::crash::tail::layer(own, in_panic_hook);
    let installed = tracing_subscriber::registry()
        .with(extra)
        .with(tail)
        .with(file.and_then(stderr).with_filter(filter))
        .try_init()
        .is_ok();
    if installed {
        if extras.panic_hook {
            install_panic_hook();
        }
        if invalid {
            tracing::warn!(
                error.type = crate::error::SharedError::InvalidLogFilter.code(),
                variable = VARIABLE,
                "the log filter is not valid; the default filter is used instead"
            );
        }
    }
}

/// Whether stderr has a layer: when it has not, a panic still shows its place there (the hook's own line).
static STDERR_LAYER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the process is, as OpenTelemetry's resource conventions name it: a closed list set here, logged once in the start
/// event and not repeated on each line. No detectors: they add host names, command lines and owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    pub service_name: &'static str,
    pub service_version: &'static str,
    pub os_type: &'static str,
    pub os_name: &'static str,
    pub host_arch: &'static str,
}

impl Resource {
    /// `os` and `arch` as Rust names them, in OpenTelemetry's values (`os.name` tells iOS and Android apart from their
    /// kernel's `os.type`); a name Rust gives that is not mapped here travels as it is.
    pub fn of(name: &'static str, version: &'static str, os: &'static str, arch: &'static str) -> Resource {
        Resource {
            service_name: name,
            service_version: version,
            os_type: match os {
                "macos" | "ios" => "darwin",
                "android" => "linux",
                other => other,
            },
            os_name: match os {
                "macos" => "macOS",
                "ios" => "iOS",
                "android" => "Android",
                "linux" => "Linux",
                "windows" => "Windows",
                other => other,
            },
            host_arch: match arch {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                other => other,
            },
        }
    }
}

/// The resource the panic record repeats the version of.
static RESOURCE: std::sync::OnceLock<Resource> = std::sync::OnceLock::new();

/// The process's start event, once, after [`init`]: the resource attributes, and the version the panic record repeats.
pub fn announce(resource: Resource) {
    let _ = RESOURCE.set(resource);
    tracing::info!(
        service.name = resource.service_name,
        service.version = resource.service_version,
        os.type = resource.os_type,
        os.name = resource.os_name,
        host.arch = resource.host_arch,
        "the application started"
    );
}

/// A panic as the crash reporter is told of it: only what may be kept (guideline section 9, `crash`).
pub struct Panicked<'a> {
    /// The payload when it is a `&'static str`; `None` for anything formatted or not text.
    pub literal: Option<&'a str>,
    /// The byte length of a formatted (`String`) payload, whose text goes no further than the hook; 0 otherwise.
    pub formatted_len: usize,
    /// Where it was raised (`code.file.path`, `code.line.number`): empty and 0 when the runtime says none.
    pub file: &'a str,
    pub line: u32,
}

/// What hears of each panic once the log has it.
type Observer = Box<dyn Fn(&Panicked<'_>) + Send + Sync>;
static OBSERVER: std::sync::OnceLock<Observer> = std::sync::OnceLock::new();

/// Has `observer` told of every panic from now on (once per process; the crash reporter). It runs inside the panic hook: it
/// must not panic or wait for a lock the panicking thread may hold.
pub fn observe_panics(observer: impl Fn(&Panicked<'_>) + Send + Sync + 'static) {
    let _ = OBSERVER.set(Box::new(observer));
}

/// Tells the observer of a panic, from the panic hook: the literal, never a formatted message.
fn tell_observer(payload: &(dyn std::any::Any + Send), location: Option<(&str, u32)>) {
    if let Some(observer) = OBSERVER.get() {
        let (literal, formatted_len) = match (payload.downcast_ref::<&'static str>(), payload.downcast_ref::<String>())
        {
            (Some(literal), _) => (Some(*literal), 0),
            (None, Some(formatted)) => (None, formatted.len()),
            (None, None) => (None, 0),
        };
        let (file, line) = location.unwrap_or(("", 0));
        observer(&Panicked { literal, formatted_len, file, line });
    }
}

/// The home directory the writers redact, for whoever else must keep it out of what it stores (the crash record).
pub fn home() -> Home {
    HOME.get().cloned().unwrap_or_default()
}

/// A panic goes to the log as two events (guideline section 9), through the subscriber: first `panic`, then `crash_backtrace`
/// for the app file only. The default hook is not chained: it would write the raw message to stderr, past the writer's
/// backstop.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        IN_HOOK.with(|flag| flag.set(true));
        record_panic(info.payload(), info.location().map(|at| (at.file(), at.line())), std::thread::current().name());
        // Whoever reports crashes (src/crash) hears of it with what may be sent: the literal, never a formatted message.
        tell_observer(info.payload(), info.location().map(|at| (at.file(), at.line())));
        IN_HOOK.with(|flag| flag.set(false));
        // However the log is set up, a panic must not vanish: with no stderr layer its place, never its message, goes there.
        if !STDERR_LAYER.load(std::sync::atomic::Ordering::Relaxed) {
            let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let location = HOME.get().map_or(location.clone(), |home| home.redact(&location));
            let _ = writeln!(std::io::stderr().lock(), "sidevoice: panic at {location}");
        }
    }));
}

/// The panic's two events. `panic`: where, which thread, which version and the message only when the payload is a literal
/// (`&'static str`); a formatted `String` is replaced by a marker and its length, never its text, a prefix or a hash,
/// because `unwrap()` and `assert_eq!` put values in it. Then `crash_backtrace`: the frames, in one line (symbols and
/// lines, no values).
fn record_panic(payload: &(dyn std::any::Any + Send), location: Option<(&str, u32)>, thread: Option<&str>) {
    let (message, len) = match (payload.downcast_ref::<&'static str>(), payload.downcast_ref::<String>()) {
        (Some(literal), _) => (*literal, None),
        (None, Some(formatted)) => (FORMATTED_MARKER, Some(formatted.len())),
        (None, None) => (NON_TEXT_MARKER, None),
    };
    let (file, line) = location.unzip();
    tracing::event!(
        target: "panic",
        tracing::Level::ERROR,
        exception.type = "panic",
        exception.message = message,
        len,
        code.file.path = file,
        code.line.number = line,
        thread.name = thread.unwrap_or("unnamed"),
        service.version = RESOURCE.get().map_or("unknown", |resource| resource.service_version),
        "the program panicked; the work of that thread was abandoned"
    );
    let backtrace = std::backtrace::Backtrace::force_capture();
    tracing::event!(
        target: "crash_backtrace",
        tracing::Level::ERROR,
        exception.type = "panic",
        exception.stacktrace = ?backtrace,
        "the backtrace of the panic recorded just before"
    );
}

/// A code a platform or a caller named (an error's key, a refusal's reason), as a log field: as it is when it is 1 to 64 of
/// `[a-z0-9_.-]` (guideline section 6), else `invalid`.
pub fn checked_code(code: &str) -> &str {
    let plain = (1..=64).contains(&code.len())
        && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b));
    if plain {
        code
    } else {
        "invalid"
    }
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
            skip_backtrace: false,
        };
        let mut writer = lines.make_writer();
        writer.write_all(b"one /HOME/ana/x ").unwrap();
        writer.write_all(b"line\n").unwrap();
        drop(writer);
        assert_eq!(*seen.lock().unwrap(), ["one ~/x line"]);
    }

    #[test]
    fn the_resource_is_a_closed_list_in_opentelemetry_values() {
        let mac = Resource::of("app", "1.0.0", "macos", "aarch64");
        assert_eq!((mac.os_type, mac.os_name, mac.host_arch), ("darwin", "macOS", "arm64"));
        let ios = Resource::of("app", "1.0.0", "ios", "aarch64");
        assert_eq!((ios.os_type, ios.os_name), ("darwin", "iOS"));
        let android = Resource::of("app", "1.0.0", "android", "aarch64");
        assert_eq!((android.os_type, android.os_name), ("linux", "Android"));
        let linux = Resource::of("app", "1.0.0", "linux", "x86_64");
        assert_eq!((linux.os_type, linux.os_name, linux.host_arch), ("linux", "Linux", "amd64"));
        let windows = Resource::of("app", "1.0.0", "windows", "x86_64");
        assert_eq!((windows.os_type, windows.os_name), ("windows", "Windows"));
        assert_eq!((mac.service_name, mac.service_version), ("app", "1.0.0"));
    }

    #[test]
    fn the_backtrace_target_is_matched_like_a_filter_directive_and_shares_no_prefix_with_the_panic_target() {
        assert!(is_backtrace_target("crash_backtrace") && is_backtrace_target("crash_backtrace::x"));
        assert!(!is_backtrace_target("crash_backtrace_x") && !is_backtrace_target("panic"));
        assert!(!PANIC_TARGET.starts_with(BACKTRACE_TARGET) && !BACKTRACE_TARGET.starts_with(PANIC_TARGET));
    }

    #[test]
    fn a_code_is_plain_or_invalid() {
        assert_eq!(checked_code("call-refused"), "call-refused");
        for hostile in ["", "Not Visible", "a:b", "x\nforged", &"a".repeat(65)] {
            assert_eq!(checked_code(hostile), "invalid");
        }
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
