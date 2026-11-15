//! iOS: `tracing-oslog`, subsystem `ai.sidevoice`, one category. It writes every line as public, which is acceptable only
//! because our lines are masked at the source and the filter lets through `warn` and `error` of our targets (`system_log`).
use sidevoice_shared::logging::Home;
use sidevoice_shared::system_log::SafeSpans;
use tracing_oslog::OsLogger;
use tracing_subscriber::{Layer, Registry};

use super::Os;

const SUBSYSTEM: &str = "ai.sidevoice";
const CATEGORY: &str = "app";

pub struct Native;

impl Os for Native {
    fn layer(_home: Home) -> Box<dyn Layer<Registry> + Send + Sync> {
        // `SafeSpans`: the layer expects the current span of an event to be one it was told of, which a filter can break.
        Box::new(SafeSpans(OsLogger::new(SUBSYSTEM, CATEGORY)))
    }
}
