//! The page's side of what every app shares, as the code the apps put in their pages (docs/BRIDGE.md -> "Parts").

/// The members of `host` that every app shares, whatever shell it is made with: the engine, the voice call, the
/// microphone's permission, the pairings and the room's call state (`js/members.js`). One function expression,
/// `(win, transport) => members`, which the app evaluates in the page's script. It names no shell: the commands reach
/// the app through the `transport` it is handed (the shell binding makes it), and the tests run it against one of
/// their own.
pub const MEMBERS: &str = include_str!("../js/members.js");
