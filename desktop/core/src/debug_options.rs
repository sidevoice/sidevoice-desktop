//! The developer options of a nightly or a dev build (`window.__sidevoiceDesktop.host.debug`, docs/BRIDGE.md → "The
//! developer options"): three switches the room's hidden "Developer options" section turns, kept in the settings and
//! off by default. The shell puts them on the webviews (src/debug_options.rs); this is what they are, which this
//! platform has, and what the page is told.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The switches, as the settings keep them. Each is off by default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugOptions {
    /// Safari's Web Inspector can be attached to the room and to the call controls card (`WKWebView.isInspectable`).
    #[serde(default)]
    pub inspectable: bool,
    /// WebKit outlines each composited layer: what the GPU draws (`_setCompositingBordersVisible:`).
    #[serde(default)]
    pub compositing_borders: bool,
    /// WebKit counts each layer's repaints on it: what the CPU keeps painting (`_setCompositingRepaintCountersVisible:`).
    #[serde(default)]
    pub repaint_counters: bool,
}

/// One switch, by the name the page uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    Inspectable,
    CompositingBorders,
    RepaintCounters,
}

impl Switch {
    pub const ALL: [Switch; 3] = [Switch::Inspectable, Switch::CompositingBorders, Switch::RepaintCounters];

    pub fn name(self) -> &'static str {
        match self {
            Switch::Inspectable => "inspectable",
            Switch::CompositingBorders => "compositingBorders",
            Switch::RepaintCounters => "repaintCounters",
        }
    }

    pub fn parse(name: &str) -> Option<Switch> {
        Switch::ALL.into_iter().find(|s| s.name() == name)
    }
}

impl DebugOptions {
    pub fn get(&self, switch: Switch) -> bool {
        match switch {
            Switch::Inspectable => self.inspectable,
            Switch::CompositingBorders => self.compositing_borders,
            Switch::RepaintCounters => self.repaint_counters,
        }
    }

    pub fn set(&mut self, switch: Switch, on: bool) {
        match switch {
            Switch::Inspectable => self.inspectable = on,
            Switch::CompositingBorders => self.compositing_borders = on,
            Switch::RepaintCounters => self.repaint_counters = on,
        }
    }

    /// What the page is told: each switch's state, and which ones this platform has (`available`), the others being
    /// hidden by the page. A switch this platform lacks always reads `false`.
    pub fn state(&self, available: &[Switch]) -> Value {
        let mut state = json!({ "available": available.iter().map(|s| s.name()).collect::<Vec<_>>() });
        for switch in Switch::ALL {
            state[switch.name()] = Value::Bool(available.contains(&switch) && self.get(switch));
        }
        state
    }
}

/// Whether a build offers the developer options: a nightly and a dev build, never a release.
pub fn offered(channel: &str) -> bool {
    channel != "release"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_release_has_no_developer_options() {
        assert!(offered("nightly") && offered("dev"));
        assert!(!offered("release"));
    }

    #[test]
    fn names_round_trip_and_nothing_else_parses() {
        for switch in Switch::ALL {
            assert_eq!(Switch::parse(switch.name()), Some(switch));
        }
        assert_eq!(Switch::parse("webInspector"), None);
        assert_eq!(Switch::parse(""), None);
    }

    #[test]
    fn the_state_says_each_switch_and_hides_what_the_platform_lacks() {
        let mut options = DebugOptions::default();
        options.set(Switch::CompositingBorders, true);
        options.set(Switch::Inspectable, true);
        assert_eq!(
            options.state(&Switch::ALL),
            json!({"available": ["inspectable", "compositingBorders", "repaintCounters"],
                   "inspectable": true, "compositingBorders": true, "repaintCounters": false})
        );
        assert_eq!(
            options.state(&[]),
            json!({"available": [], "inspectable": false, "compositingBorders": false, "repaintCounters": false})
        );
    }

    #[test]
    fn all_off_by_default_and_a_missing_field_reads_off() {
        assert_eq!(serde_json::from_value::<DebugOptions>(json!({})).unwrap(), DebugOptions::default());
        let kept: DebugOptions = serde_json::from_value(json!({"repaintCounters": true})).unwrap();
        assert!(kept.repaint_counters && !kept.inspectable && !kept.compositing_borders);
    }
}
