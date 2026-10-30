//! How much the app log says: one environment variable, no setting in the app.

use log::LevelFilter;

/// The variable that sets the app log's level (`error`, `warn`, `info`, `debug`, `trace` or `off`).
pub const LEVEL_VARIABLE: &str = "SIDEVOICE_LOG_LEVEL";

/// The level the variable asks for; `info` when it is unset, empty or not a level.
pub fn level(value: Option<&str>) -> LevelFilter {
    value.and_then(|text| text.trim().parse().ok()).unwrap_or(LevelFilter::Info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_defaults_to_info() {
        for value in [None, Some(""), Some("loud"), Some(" ")] {
            assert_eq!(level(value), LevelFilter::Info, "{value:?}");
        }
    }

    #[test]
    fn the_variable_names_a_level_in_any_case() {
        assert_eq!(level(Some("debug")), LevelFilter::Debug);
        assert_eq!(level(Some(" WARN ")), LevelFilter::Warn);
        assert_eq!(level(Some("off")), LevelFilter::Off);
    }
}
