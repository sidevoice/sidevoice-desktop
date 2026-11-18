// Sidevoice: the page's problems in the app's log (sidevoice-shared-tauri's `webview_log` command; `js::WEBVIEW_LOG`), injected by
// each app into the window that shows the room (the desktop's room window, the phone's one window). Forwards `console.warn` / `console.error`, uncaught errors and unhandled rejections to
// that command, and leaves the console as it was.
//
// The enforcing allowlist is the Rust side's (shared/tauri/src/webview_log.rs: a page is not trusted); this script only
// sends the allowed shapes, never free text (a log line is written as if it will be published): numbers, booleans and
// null; for an Error or any other object, its `name` (only `...Error` or `...Exception`), its `code` and `key` (only a dotted
// code such as `voice.failure.model`) and the first frame of its stack as file, line and column. Every string, an Error's
// `message` included, is sent as `{kind: "string"}`. A flood is dropped (the Rust side counts it).
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window);
})(function installWebviewLog(win) {
  "use strict";
  const MAX_PER_WINDOW = 50; // entries per 10 s (the Rust side limits too)
  const WINDOW_MS = 10000;
  const MAX_ITEMS = 16;
  const NAME = /^[A-Za-z]{1,40}(Error|Exception)$/;
  const KEY = /^[a-z][a-z0-9_]{0,31}(\.[a-z0-9_]{1,32}){2,4}$/; // a dotted code of three parts: voice.failure.model
  const PREFIX = /^(voice|core|engine|app|call)\.[a-z0-9_]{1,32}(\.[a-z0-9_]{1,32}){0,3}$/; // or one of ours: core.refused
  const isCode = (v) => KEY.test(v) || PREFIX.test(v);
  const KEY_FILE = /^[A-Za-z0-9_.-]{1,64}$/;
  const FRAME = /([A-Za-z0-9_.-]{1,64}):(\d{1,7})(?::(\d{1,7}))?\)?\s*$/;

  // The first frame of a stack, from a line that is one: WebKit's `fn@file:line:col` or V8's `at fn (file:line:col)`. A
  // message that spills onto lines of its own is not a frame, so only those shapes count.
  function frameOf(stack) {
    if (typeof stack !== "string") return undefined;
    for (const line of stack.split("\n").slice(0, 6)) {
      const v8 = line.trimStart().startsWith("at ") && (line.trimEnd().endsWith(")") || line.includes("://"));
      if (!line.includes("@") && !v8) continue;
      const found = FRAME.exec(line);
      if (found) {
        const frame = { file: found[1], line: Number(found[2]) };
        if (found[3]) frame.column = Number(found[3]);
        return frame;
      }
    }
    return undefined;
  }

  // One value as the shape the Rust side allows.
  function describe(value) {
    if (value === null) return { kind: "null" };
    if (value === undefined) return { kind: "undefined" };
    if (typeof value === "number") return Number.isFinite(value) ? { kind: "number", value } : { kind: "other" };
    if (typeof value === "boolean") return { kind: "bool", value };
    if (typeof value === "string") return { kind: "string" };
    if (typeof value !== "object") return { kind: "other" };
    const item = { kind: "object" };
    if (typeof value.name === "string" && NAME.test(value.name)) item.name = value.name;
    if (typeof value.code === "string" && isCode(value.code)) item.code = value.code;
    if (typeof value.key === "string" && isCode(value.key)) item.key = value.key;
    const at = frameOf(value.stack);
    if (at) item.at = at;
    return item;
  }

  let windowStart = 0;
  let sent = 0;

  function send(level, source, items, at) {
    const internals = win.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== "function") return;
    const now = Date.now();
    if (now - windowStart > WINDOW_MS) {
      windowStart = now;
      sent = 0;
    }
    if (sent >= MAX_PER_WINDOW) return;
    sent += 1;
    const entry = { level, source, items: items.slice(0, MAX_ITEMS) };
    if (at) entry.at = at;
    // The log must never break the page: the capability may not match yet, or the app may be reloading.
    try {
      Promise.resolve(internals.invoke("plugin:sidevoice-shared-tauri|webview_log", { entry })).catch(() => {});
    } catch (_e) {
      /* ignored */
    }
  }

  for (const method of ["warn", "error"]) {
    const original = win.console && win.console[method];
    if (typeof original !== "function") continue;
    win.console[method] = function (...args) {
      try {
        send(method, "console", args.slice(0, MAX_ITEMS).map(describe));
      } catch (_e) {
        /* ignored */
      }
      return original.apply(this, args);
    };
  }

  win.addEventListener("error", (event) => {
    try {
      const base = event.filename ? String(event.filename).split("/").pop() : "";
      const at = KEY_FILE.test(base) ? { file: base, line: Number(event.lineno) || 0, column: Number(event.colno) || 0 } : undefined;
      send("error", "uncaught", [describe(event.error || event.message)], at);
    } catch (_e) {
      /* ignored */
    }
  });
  win.addEventListener("unhandledrejection", (event) => {
    try {
      send("error", "rejection", [describe(event.reason)]);
    } catch (_e) {
      /* ignored */
    }
  });
});
