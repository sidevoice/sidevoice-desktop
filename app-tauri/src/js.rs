//! The page's side of the plugin, as the code an app puts in its pages (docs/BRIDGE.md -> "Parts").

/// How a page reaches the plugin's commands and receives the messages of the channel it opened (`js/transport.js`): the
/// transport sidevoice-app-core's members (`sidevoice_app_core::js::MEMBERS`) are handed. One function expression,
/// `(win) => transport`. The commands are invoked under the plugin's name, [`crate::PLUGIN`]; the app's own are not.
pub const TRANSPORT: &str = include_str!("../js/transport.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transport_names_this_plugin() {
        assert!(TRANSPORT.contains(&format!("\"plugin:{}|\"", crate::PLUGIN)));
    }
}
