//! The page's side of the plugin, as the code an app puts in its pages (docs/BRIDGE.md -> "Parts").

/// The members of `host` that every app shares, whatever shell it is made with: the engine, the voice call, the
/// microphone's permission, the pairings and the room's call state (`js/members.js`). One function expression,
/// `(win, transport) => members`, which the app evaluates in the page's script. It names no shell: the commands reach
/// the app through the `transport` it is handed (the shell binding makes it), and the tests run it against one of
/// their own.
pub const MEMBERS: &str = include_str!("../js/members.js");

/// How a page reaches the plugin's commands and receives the messages of the channel it opened (`js/transport.js`): the
/// transport the members ([`MEMBERS`]) are handed. One function expression, `(win) => transport`. The commands are
/// invoked under the plugin's name, [`crate::PLUGIN`]; the app's own are not.
pub const TRANSPORT: &str = include_str!("../js/transport.js");

/// The page's problems in the app's log (`js/webview-log.js`): its `console.warn` and `console.error`, uncaught errors and
/// unhandled rejections, through tauri-plugin-log's `log` command (target `webview`), narrowed and masked. An app that
/// injects it registers that plugin and grants its window `log:allow-log`.
pub const WEBVIEW_LOG: &str = include_str!("../js/webview-log.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transport_names_this_plugin() {
        assert!(TRANSPORT.contains(&format!("\"plugin:{}|\"", crate::PLUGIN)));
    }
}
