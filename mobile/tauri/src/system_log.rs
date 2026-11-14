//! The app's log in the system's own: a `tracing` layer that hands each event to the phone's log (the unified log on iOS through
//! the Swift side's `SidevoiceLog`, logcat on Android: `platform::system_log`), in the category its real target names (docs:
//! the Sidevoice logging guideline, sidevoice/.github-private `docs/logging.md`, section 9).
//!
//! It is a layer and not a writer because the target is what chooses, and a writer is given the raw metadata: every record
//! that comes through the `log` bridge has the target `log` there, with the real one in a field. [`NormalizeEvent`] gives the
//! real one back. Without it every `log::info!(target: "cue", ..)` would land in the private category, silently.

use std::fmt::{self, Write as _};

use sidevoice_shared::logging::Home;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// The categories that are public in the unified log: our own targets whose lines carry only codes, counts, our own ids and
/// the system's enum names. Matched exactly, never by prefix.
const PUBLIC: [&str; 4] = ["stills", "intent", "cue", "audio"];

/// The category (and so the privacy) of the lines with `target`: its own name when it is one of [`PUBLIC`], else `app`, which
/// is private. The page's own lines (target `webview`) are page text and always `app`.
pub fn category(target: &str) -> &'static str {
    PUBLIC
        .into_iter()
        .find(|public| *public == target)
        .unwrap_or("app")
}

/// The layer: `sink` gets the category, the level and one line, with the home directory shown as `~`.
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
    F: Fn(&'static str, Level, &str) + Send + Sync + 'static,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // A record from the `log` bridge says its real target and level in its fields; any other event is its own.
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut line = format!("[{}]", metadata.level());
        for span in ctx
            .event_scope(event)
            .into_iter()
            .flat_map(|scope| scope.from_root())
        {
            let _ = write!(line, " {}:", span.name());
        }
        if !fields.message.is_empty() {
            let _ = write!(line, " {}", fields.message);
        }
        line.push_str(&fields.rest);
        (self.sink)(
            category(metadata.target()),
            *metadata.level(),
            &self.home.redact(&line),
        );
    }
}

/// An event's fields as text: its message, and the other fields as ` key=value`. The `log` bridge's own (`log.target`,
/// `log.file`, ..) are what the layer reads back, not content.
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

    type Seen = Arc<Mutex<Vec<(&'static str, Level, String)>>>;

    /// A subscriber like the app's (the layer under `SIDEVOICE_LOG`'s filter, `log` bridged), and what its sink was given.
    fn recording(f: impl FnOnce()) -> Vec<(&'static str, Level, String)> {
        let seen: Seen = Arc::default();
        let sink = Arc::clone(&seen);
        let layer = SystemLog::new(
            Home::new(Some(std::path::Path::new("/home/ana"))),
            move |category, level, line: &str| {
                sink.lock()
                    .unwrap()
                    .push((category, level, line.to_owned()));
            },
        );
        let (filter, _) = sidevoice_shared::logging::filter(
            None,
            &["sidevoice_mobile_lib", "stills", "intent", "cue", "audio"],
        );
        let subscriber = tracing_subscriber::registry().with(layer.with_filter(filter));
        // `log` records are turned into events by hand, as `LogTracer` does, so the test owns its dispatcher.
        tracing::subscriber::with_default(subscriber, || {
            f();
        });
        let seen = seen.lock().unwrap().clone();
        seen
    }

    #[test]
    fn only_the_four_targets_are_public_and_by_their_whole_name() {
        for public in PUBLIC {
            assert_eq!(category(public), public);
        }
        for private in [
            "app",
            "log",
            "webview",
            "sidevoice_mobile_lib",
            "cue2",
            "cue::x",
            "audio_session",
            "stills ",
            "Cue",
            "",
        ] {
            assert_eq!(category(private), "app", "{private}");
        }
    }

    #[test]
    fn a_log_record_reaches_the_category_of_its_real_target() {
        let seen = recording(|| {
            for target in PUBLIC {
                tracing_log::format_trace(
                    &log::Record::builder()
                        .args(format_args!("said by {target}"))
                        .level(log::Level::Info)
                        .target(target)
                        .build(),
                )
                .unwrap();
            }
        });
        let categories: Vec<_> = seen.iter().map(|(category, _, _)| *category).collect();
        assert_eq!(categories, PUBLIC, "{seen:?}");
        assert_eq!(seen[0].2, "[INFO] said by stills");
    }

    #[test]
    fn other_lines_are_private() {
        let seen = recording(|| {
            for target in ["webview", "sidevoice_mobile_lib", "tao"] {
                let record = log::Record::builder()
                    .args(format_args!("a line"))
                    .level(log::Level::Warn)
                    .target(target)
                    .build();
                tracing_log::format_trace(&record).unwrap();
            }
            tracing::warn!("a native event");
            tracing::info!(target: "cue", "a native cue event");
        });
        let categories: Vec<_> = seen.iter().map(|(category, _, _)| *category).collect();
        assert_eq!(categories, ["app", "app", "app", "app", "cue"], "{seen:?}");
    }

    #[test]
    fn a_line_shows_its_fields_and_spans_and_hides_the_home() {
        let seen = recording(|| {
            let span = tracing::info_span!(target: "audio", "call");
            let _entered = span.enter();
            tracing::warn!(target: "audio", count = 3, "route /home/ana/x");
        });
        assert_eq!(
            seen,
            [(
                "audio",
                Level::WARN,
                "[WARN] call: route ~/x count=3".to_owned()
            )]
        );
    }
}
