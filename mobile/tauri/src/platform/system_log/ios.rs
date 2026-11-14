//! iOS: `SidevoiceLog` (Swift), subsystem `ai.sidevoice`, the category given; the level is in the line's own `[LEVEL]`.
use tracing::Level;

use super::Os;

pub struct Native;

impl Os for Native {
    fn write(category: &str, _level: Level, line: &str) {
        sidevoice_mobile_plugin::os_log(category, line);
    }
}
