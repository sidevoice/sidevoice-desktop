// Sidevoice: the page's problems in the app's log (tauri-plugin-log; sidevoice-shared-tauri's `js::WEBVIEW_LOG`), injected by
// each app into the window that shows the room (the desktop's room window, the phone's one window). Forwards `console.warn` / `console.error`, uncaught errors and unhandled rejections to the plugin's
// `log` command, and leaves the console as it was.
//
// What it writes is an allowlist, never free text (a log line is written as if it will be published): numbers, booleans and
// null; for an Error or any other object, its `name` (only `...Error` or `...Exception`), its `code` and `key` (only a dotted
// code such as `voice.failure.model`) and
// the first frame of its stack as `file:line:column`. Every string, an Error's `message` included, is written as `[string]`.
// A flood is dropped (a count is written instead).
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window);
})(function installWebviewLog(win) {
  "use strict";
  const WARN = 4;
  const ERROR = 5;
  const MAX_PER_WINDOW = 50; // lines per 10 s
  const WINDOW_MS = 10000;
  const NAME = /^[A-Za-z]{1,40}(Error|Exception)$/;
  const KEY = /^[a-z][a-z0-9_]{0,31}(\.[a-z0-9_]{1,32}){1,4}$/; // a dotted code: voice.failure.model
  const KEY_FILE = /^[A-Za-z0-9_.-]{1,64}$/;
  const FRAME = /([A-Za-z0-9_.-]{1,64}):(\d{1,7})(?::(\d{1,7}))?\)?\s*$/;

  // The first frame of a stack, from a line that is one: WebKit's `fn@file:line:col` or V8's `at fn (file:line:col)`. A
  // message that spills onto lines of its own is not a frame, so only those shapes count.
  function frameOf(stack) {
    if (typeof stack !== "string") return "";
    for (const line of stack.split("\n").slice(0, 6)) {
      const v8 = line.trimStart().startsWith("at ") && (line.trimEnd().endsWith(")") || line.includes("://"));
      if (!line.includes("@") && !v8) continue;
      const found = FRAME.exec(line);
      if (found) return found[1] + ":" + found[2] + (found[3] ? ":" + found[3] : "");
    }
    return "";
  }

  function describe(value) {
    if (value === null || value === undefined || typeof value === "number" || typeof value === "boolean") return String(value);
    if (typeof value === "string") return "[string]";
    if (typeof value !== "object") return `[${typeof value}]`;
    const picked = [];
    for (const [field, shape] of [["name", NAME], ["code", KEY], ["key", KEY]]) {
      const v = value[field];
      if (typeof v === "string" && shape.test(v)) picked.push(`${field}=${v}`);
    }
    const frame = frameOf(value.stack);
    if (frame) picked.push(`at=${frame}`);
    return picked.length ? `{${picked.join(" ")}}` : "[object]";
  }

  let windowStart = 0;
  let sent = 0;
  let dropped = 0;

  function send(level, text) {
    const internals = win.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") return;
    const now = Date.now();
    if (now - windowStart > WINDOW_MS) {
      const lost = dropped;
      windowStart = now;
      sent = 0;
      dropped = 0;
      if (lost > 0) send(WARN, `webview log: ${lost} lines dropped (flood)`);
    }
    if (sent >= MAX_PER_WINDOW) {
      dropped += 1;
      return;
    }
    sent += 1;
    // The log must never break the page: the capability may not match yet, or the app may be reloading.
    try {
      Promise.resolve(internals.invoke("plugin:log|log", { level, message: text })).catch(() => {});
    } catch (_e) {
      /* ignored */
    }
  }

  for (const [method, level] of [["warn", WARN], ["error", ERROR]]) {
    const original = win.console && win.console[method];
    if (typeof original !== "function") continue;
    win.console[method] = function (...args) {
      try {
        send(level, `console.${method}: ${args.map(describe).join(" ")}`);
      } catch (_e) {
        /* ignored */
      }
      return original.apply(this, args);
    };
  }

  win.addEventListener("error", (event) => {
    try {
      const base = event.filename ? String(event.filename).split("/").pop() : "";
      const where = KEY_FILE.test(base) ? ` at ${base}:${event.lineno}:${event.colno}` : "";
      send(ERROR, `uncaught: ${event.error ? describe(event.error) : describe(event.message)}${where}`);
    } catch (_e) {
      /* ignored */
    }
  });
  win.addEventListener("unhandledrejection", (event) => {
    try {
      send(ERROR, `unhandled rejection: ${describe(event.reason)}`);
    } catch (_e) {
      /* ignored */
    }
  });
});
