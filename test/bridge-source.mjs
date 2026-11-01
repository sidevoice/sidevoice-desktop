// The page's script as the tests run it: the three parts the app composes (docs/BRIDGE.md -> "Parts"), each read from the
// crate that owns it. `desktop-bridge.js` is the desktop's part; the members and the transport are the apps' and the
// shell binding's. The composition is the one `compose` makes in desktop/core/src/bridge.rs, which has a test of its shape.
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");

export const transport = read("../app-tauri/js/transport.js");
export const members = read("../app-core/js/members.js");
export const desktop = read("../bridge/desktop-bridge.js");

/** The script the page gets, with the desktop's placeholders still in place. */
export const composed = `(function (transportOf, membersOf) {\n${desktop}\n})(${transport}, ${members});`;
