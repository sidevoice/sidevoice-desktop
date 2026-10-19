// Sidevoice desktop: the room page's problems in the app log (src-tauri/src/lib.rs, tauri-plugin-log). Injected into the
// room window only. Forwards `console.warn` / `console.error`, uncaught errors and unhandled rejections to the plugin's
// `log` command, and leaves the console as it was.
//
// What it writes is deliberately narrow: strings and numbers as given, an Error as `name: message` plus the first lines
// of its stack, and any other object only by its `code`, `key`, `name` and `message` fields (the failure codes the page
// shows). Never an object's other fields: they may hold a transcript or a message. Credential-shaped text is masked, each
// line is cut short, and a flood is dropped (a count is written instead).
(function (factory) {
  if (typeof module === "object" && module.exports) module.exports = factory; // unit tests
  else factory(window);
})(function installWebviewLog(win) {
  "use strict";
  const WARN = 4;
  const ERROR = 5;
  const MAX_LINE = 1000;
  const MAX_PER_WINDOW = 50; // lines per 10 s
  const WINDOW_MS = 10000;
  const SECRET = [
    /\b(sk|pk|rk|xai|gsk|ghp|gho|github_pat)[-_][A-Za-z0-9_-]{8,}/g,
    /\bBearer\s+[A-Za-z0-9._~+/=-]{8,}/gi,
    /\b(api[-_]?key|token|secret|password|authorization|pairing[-_ ]?code|code(?=["']?\s*[:=]\s*["']?\d))(["']?\s*[:=]\s*["']?)[^\s"',;&}]+/gi,
    /\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]*/g,
  ];

  function mask(text) {
    let out = String(text);
    out = out.replace(SECRET[0], "[redacted]").replace(SECRET[1], "Bearer [redacted]");
    out = out.replace(SECRET[2], (_all, name, sep) => name + sep + "[redacted]");
    return out.replace(SECRET[3], "[redacted]");
  }

  function describe(value) {
    if (value === null || value === undefined) return String(value);
    if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") return String(value);
    if (value instanceof Error || (typeof value === "object" && typeof value.stack === "string" && typeof value.message === "string")) {
      const stack = typeof value.stack === "string" ? value.stack.split("\n").slice(0, 4).join(" | ") : "";
      return `${value.name}: ${value.message}${stack ? ` (${stack})` : ""}`;
    }
    if (typeof value === "object") {
      const picked = [];
      for (const field of ["code", "key", "name", "message"]) {
        const v = value[field];
        if (typeof v === "string" || typeof v === "number") picked.push(`${field}=${v}`);
      }
      return picked.length ? `{${picked.join(" ")}}` : "[object]";
    }
    return `[${typeof value}]`;
  }

  let windowStart = 0;
  let sent = 0;
  let dropped = 0;

  function send(level, message) {
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
    let text = mask(message);
    if (text.length > MAX_LINE) text = `${text.slice(0, MAX_LINE)}...`;
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
      const where = event.filename ? ` at ${String(event.filename).split("/").pop()}:${event.lineno}:${event.colno}` : "";
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
