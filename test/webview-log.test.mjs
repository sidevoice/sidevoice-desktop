// Unit tests of shared/tauri/js/webview-log.js against a fake window.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../shared/tauri/js/webview-log.js", import.meta.url), "utf8");

function setup() {
  const module = { exports: {} };
  vm.runInNewContext(source, { module });
  const calls = [];
  const listeners = {};
  const original = [];
  const win = {
    console: { warn: (...a) => original.push(["warn", ...a]), error: (...a) => original.push(["error", ...a]) },
    addEventListener: (type, fn) => (listeners[type] = fn),
    __TAURI_INTERNALS__: { invoke: (command, args) => (calls.push({ command, ...args }), Promise.resolve()) },
  };
  module.exports(win);
  return { win, calls, listeners, original };
}

test("console.error and console.warn reach the plugin and still reach the console", () => {
  const { win, calls, original } = setup();
  win.console.error("boom", 3);
  win.console.warn("careful");
  assert.deepEqual(calls.map((c) => [c.command, c.level, c.message]), [
    ["plugin:log|log", 5, "console.error: [string] 3"],
    ["plugin:log|log", 4, "console.warn: [string]"],
  ]);
  assert.equal(original.length, 2);
});

test("objects are written by their allowlisted fields only", () => {
  const { win, calls } = setup();
  win.console.error({ code: "voice.failure.model", transcript: "private words", message: "private words" });
  assert.equal(calls[0].message, "console.error: {code=voice.failure.model}");
  win.console.error({ transcript: "private words" });
  assert.equal(calls[1].message, "console.error: [object]");
  win.console.error({ code: "has spaces and private words", key: "Not_A_Key" });
  assert.equal(calls[2].message, "console.error: [object]");
});

test("free text never reaches the log, whatever it looks like", () => {
  const { win, calls } = setup();
  win.console.error("failed with sk-abcdefghijkl and Bearer abcdefgh12345 api_key=hunter22hunter");
  win.console.error(new Error("canary words 7f3a"), { message: "canary words 7f3a" });
  for (const call of calls) assert.ok(!/abcdefghijkl|abcdefgh12345|hunter22|canary/.test(call.message), call.message);
});

test("an Error is written by its name and the first frame of its stack", () => {
  const { win, calls } = setup();
  const error = { name: "TypeError", message: "canary words 7f3a", stack: "TypeError: canary words 7f3a\nrender@http://localhost/assets/app.js:12:34\nother@http://x/b.js:1:1" };
  win.console.error(error);
  assert.equal(calls[0].message, "console.error: {name=TypeError at=app.js:12:34}");
});

test("uncaught errors and unhandled rejections are logged", () => {
  const { listeners, calls } = setup();
  listeners.error({ error: new TypeError("bad"), filename: "http://x/app.js?token=canary", lineno: 1, colno: 2 });
  listeners.error({ message: "canary words", filename: "http://x/app.js", lineno: 1, colno: 2 });
  listeners.unhandledrejection({ reason: { key: "voice.failure.start" } });
  assert.match(calls[0].message, /^uncaught: \{name=TypeError[^}]*\}$/);
  assert.ok(!calls[0].message.includes("canary"));
  assert.equal(calls[1].message, "uncaught: [string] at app.js:1:2");
  assert.equal(calls[2].message, "unhandled rejection: {key=voice.failure.start}");
});

test("a flood is cut", () => {
  const { win, calls } = setup();
  for (let i = 0; i < 100; i++) win.console.error("again");
  assert.equal(calls.length, 50);
});

test("a key or code that is not a dotted code, and a name that is not an Error, are dropped", () => {
  const { win, calls } = setup();
  win.console.error({ key: "sk_abcdefghijklmnop", code: "deadbeefdeadbeef", name: "canary words" });
  win.console.error({ key: "voice.failure.model", code: "UPPER.case", name: "NotAllowed" });
  win.console.error({ name: "AbortError", code: "core.refused" });
  win.console.error({ code: "john.doe", key: "a.b" });
  assert.equal(calls[0].message, "console.error: [object]");
  assert.equal(calls[1].message, "console.error: {key=voice.failure.model}");
  assert.equal(calls[2].message, "console.error: {name=AbortError code=core.refused}");
  assert.equal(calls[3].message, "console.error: [object]", "two plain words are not a code");
});

test("only a line shaped like a frame is read as one", () => {
  const { win, calls } = setup();
  win.console.error({ stack: "Error: canary\nat canary.txt:9\nfn@http://h/app.js:3:4" });
  win.console.error({ stack: "Error: x\n    at render (http://h/main.js:7:8)" });
  win.console.error({ stack: "canary spill\nmore words.txt:5" });
  assert.equal(calls[0].message, "console.error: {at=app.js:3:4}");
  assert.equal(calls[1].message, "console.error: {at=main.js:7:8}");
  assert.equal(calls[2].message, "console.error: [object]");
});
