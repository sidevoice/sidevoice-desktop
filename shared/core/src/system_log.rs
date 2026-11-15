//! The app's log in the system's own, for the phone (docs: the Sidevoice logging guideline, sidevoice/.github-private
//! `docs/logging.md`, section 8). Our lines are masked at the source, so the system log carries the same text as the app's
//! file, but only `warn` and `error` of our own targets: [`filter`] is that rule, and it is not `SIDEVOICE_LOG`'s, so no
//! setting can widen it. A dependency's line (tauri, wry, hyper, anything bridged from `log`) carries URLs, error chains and
//! paths we do not control, and the backtrace of a panic carries the program's shape: neither reaches the system's log.
//!
//! iOS uses `tracing-oslog`'s layer, wrapped in [`SafeSpans`]; Android uses [`SystemLog`], which hands each line to logcat.

use std::fmt::{self, Write as _};

use crate::logging::{is_backtrace_target, Home, BACKTRACE_TARGET};
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::{Context, Filter};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// Note for iOS: `tracing-oslog` writes the names and fields of every admitted span (`target::name(field: value)`) into the
/// public activity name from DEBUG up, so the span fields of our targets must only hold values allowed in the system log.
///
/// The most a span of ours may be verbose to be seen by the system layer. A layer that builds something for each span (iOS's
/// `os_activity`) needs the spans an event is in; the cost is held by taking none of `trace`, which is where the hot paths are.
const SPAN_LEVEL: LevelFilter = LevelFilter::DEBUG;

/// What the system log takes: events of `own` at `warn` and `error`, and their spans; nothing of any other target, and the
/// backtrace event never (written out, and not by omission: filters match targets by prefix).
pub fn filter(own: &[&str]) -> SystemFilter {
    let targets = |level: LevelFilter| {
        own.iter()
            .filter(|target| !is_backtrace_target(target))
            .fold(Targets::new().with_default(LevelFilter::OFF), |targets, target| targets.with_target(*target, level))
            .with_target(BACKTRACE_TARGET, LevelFilter::OFF)
    };
    SystemFilter { events: targets(LevelFilter::WARN), spans: targets(SPAN_LEVEL) }
}

/// The per-layer filter of [`filter`]. Events and spans are told apart because a layer that keeps state per span (iOS's) must
/// see the spans its events are in, whatever the event's own level.
pub struct SystemFilter {
    events: Targets,
    spans: Targets,
}

impl SystemFilter {
    fn takes(&self, metadata: &Metadata<'_>) -> bool {
        if is_backtrace_target(metadata.target()) {
            return false;
        }
        let targets = if metadata.is_span() { &self.spans } else { &self.events };
        targets.would_enable(metadata.target(), metadata.level())
    }
}

impl<S> Filter<S> for SystemFilter {
    fn enabled(&self, metadata: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        self.takes(metadata)
    }

    // The rule holds no state: a callsite is decided once, and a disabled hot one costs nothing afterwards.
    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.takes(metadata) {
            Interest::always()
        } else {
            Interest::never()
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(SPAN_LEVEL)
    }
}

/// Wraps a layer that keeps something per span and expects its events' current span to be one it has seen (`tracing-oslog`
/// does: it `expect`s it, and a panic in the log would be the worst outcome). Under a per-layer filter the registry's
/// current span can be one the filter turned off (a dependency's span around a call into our code), and then such a layer
/// panics. The event is dropped here instead: it is in the app's file all the same.
pub struct SafeSpans<L>(pub L);

impl<S, L> Layer<S> for SafeSpans<L>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    L: Layer<S>,
{
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &tracing::span::Id, ctx: Context<'_, S>) {
        self.0.on_new_span(attrs, id, ctx);
    }

    fn on_record(&self, id: &tracing::span::Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        self.0.on_record(id, values, ctx);
    }

    fn on_follows_from(&self, id: &tracing::span::Id, follows: &tracing::span::Id, ctx: Context<'_, S>) {
        self.0.on_follows_from(id, follows, ctx);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if ctx.current_span().id().is_some_and(|id| ctx.span(id).is_none()) {
            return;
        }
        self.0.on_event(event, ctx);
    }

    fn on_enter(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        self.0.on_enter(id, ctx);
    }

    fn on_exit(&self, id: &tracing::span::Id, ctx: Context<'_, S>) {
        self.0.on_exit(id, ctx);
    }

    fn on_close(&self, id: tracing::span::Id, ctx: Context<'_, S>) {
        self.0.on_close(id, ctx);
    }
}

/// A layer that hands each event, as one line with the home directory shown as `~`, to `sink` with its level (Android's logcat).
pub struct SystemLog<F> {
    sink: F,
    home: Home,
}

impl<F> SystemLog<F> {
    pub fn new(home: Home, sink: F) -> SystemLog<F> {
        SystemLog { sink, home }
    }
}

impl<S, F> Layer<S> for SystemLog<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    F: Fn(Level, &str) + Send + Sync + 'static,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // A record from the `log` bridge says its real level in its fields; any other event is its own.
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut line = String::new();
        for span in ctx.event_scope(event).into_iter().flat_map(|scope| scope.from_root()) {
            let _ = write!(line, "{}: ", span.name());
        }
        line.push_str(&fields.message);
        line.push_str(&fields.rest);
        (self.sink)(*metadata.level(), &self.home.redact(&line));
    }
}

/// An event's fields as text: its message, and the other fields as ` key=value`. The `log` bridge's own (`log.target`,
/// `log.file`, ..) are what the bridge adds, not content.
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else if !field.name().starts_with("log.") {
            let _ = write!(self.rest, " {}={value:?}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    const OWN: &[&str] = &["sidevoice_mobile_lib", "stills", "panic"];

    type Seen = Arc<Mutex<Vec<(Level, String)>>>;

    /// A subscriber like the app's (the layer under the system filter, `log` bridged), and what its sink was given.
    fn recording(f: impl FnOnce()) -> Vec<(Level, String)> {
        let seen: Seen = Arc::default();
        let sink = Arc::clone(&seen);
        let layer = SystemLog::new(Home::new(Some(std::path::Path::new("/home/ana"))), move |level, line: &str| {
            sink.lock().unwrap().push((level, line.to_owned()));
        });
        let subscriber = tracing_subscriber::registry().with(layer.with_filter(filter(OWN)));
        tracing::subscriber::with_default(subscriber, f);
        let seen = seen.lock().unwrap().clone();
        seen
    }

    #[test]
    fn only_warn_and_error_of_our_targets_reach_the_system_log() {
        let seen = recording(|| {
            tracing::info!(target: "stills", "too quiet");
            tracing::debug!("too quiet");
            tracing::warn!(target: "stills", count = 3, "a still was refused");
            tracing::error!(target: "sidevoice_mobile_lib::call", "the call did not start");
            tracing::warn!(target: "sidevoice_mobile_library", "a prefix of ours is ours");
            tracing::error!(target: "hyper::client", "a dependency's line");
            tracing::warn!(target: "webview", "the page's line");
            tracing::warn!("this crate's own module path is not one of ours here");
        });
        let lines: Vec<_> = seen.iter().map(|(level, line)| (*level, line.as_str())).collect();
        assert_eq!(
            lines,
            [
                (Level::WARN, "a still was refused count=3"),
                (Level::ERROR, "the call did not start"),
                (Level::WARN, "a prefix of ours is ours"),
            ]
        );
    }

    #[test]
    fn a_log_record_of_a_dependency_is_dropped_and_one_of_ours_keeps_its_real_target() {
        let seen = recording(|| {
            for (target, level) in
                [("tao", log::Level::Error), ("stills", log::Level::Warn), ("stills", log::Level::Info)]
            {
                let record = log::Record::builder().args(format_args!("a record")).level(level).target(target).build();
                tracing_log::format_trace(&record).unwrap();
            }
        });
        assert_eq!(seen, [(Level::WARN, "a record".to_owned())]);
    }

    #[test]
    fn the_backtrace_event_never_reaches_it_whatever_the_list_holds() {
        let seen = {
            let seen: Seen = Arc::default();
            let sink = Arc::clone(&seen);
            let layer = SystemLog::new(Home::default(), move |level, line: &str| {
                sink.lock().unwrap().push((level, line.to_owned()));
            });
            let own = ["panic", BACKTRACE_TARGET, "crash_backtrace::deeper"];
            let subscriber = tracing_subscriber::registry().with(layer.with_filter(filter(&own)));
            tracing::subscriber::with_default(subscriber, || {
                tracing::error!(target: "panic", "the panic");
                tracing::error!(target: "crash_backtrace", "the frames");
                tracing::error!(target: "crash_backtrace::deeper", "more frames");
            });
            let seen = seen.lock().unwrap().clone();
            seen
        };
        assert_eq!(seen, [(Level::ERROR, "the panic".to_owned())]);
    }

    #[test]
    fn a_line_shows_its_fields_and_spans_and_hides_the_home() {
        let seen = recording(|| {
            let span = tracing::info_span!(target: "stills", "call");
            let _entered = span.enter();
            tracing::warn!(target: "stills", count = 3, "route /home/ana/x");
        });
        assert_eq!(seen, [(Level::WARN, "call: route ~/x count=3".to_owned())]);
    }

    /// What `tracing-oslog`'s layer does with its spans, as far as it can panic: it makes something for each span it is told
    /// of, and `expect`s the registry's current span to be one of them.
    struct Strict(Arc<Mutex<Vec<String>>>);

    impl<S> Layer<S> for Strict
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
            if let Some(id) = ctx.current_span().id() {
                ctx.span(id).expect("invalid span, this shouldn't happen");
            }
            let mut fields = Fields::default();
            event.record(&mut fields);
            self.0.lock().unwrap().push(fields.message);
        }
    }

    #[test]
    fn an_event_inside_a_span_the_filter_turned_off_is_dropped_not_a_panic() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let layer = SafeSpans(Strict(Arc::clone(&seen))).with_filter(filter(OWN));
        // Another layer that wants every span (the app's file layer wants a dependency's warnings): the spans exist, and
        // this layer's filter has turned some of them off.
        struct Everything;
        impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Everything {}
        let subscriber = tracing_subscriber::registry().with(Everything).with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(target: "stills", "no span");
            tracing::info_span!(target: "stills", "ours").in_scope(|| {
                tracing::warn!(target: "stills", "inside our span");
                tracing::info_span!(target: "hyper", "theirs").in_scope(|| {
                    tracing::error!(target: "stills", "inside a dependency's span");
                    tracing::info_span!(target: "stills", "ours again").in_scope(|| {
                        tracing::error!(target: "stills", "inside ours, inside theirs");
                    });
                });
            });
            tracing::trace_span!(target: "stills", "hot").in_scope(|| {
                tracing::warn!(target: "stills", "inside a span too verbose for the layer");
            });
        });
        assert_eq!(*seen.lock().unwrap(), ["no span", "inside our span", "inside ours, inside theirs"]);
    }
}
