// Unit tests of bridge/webview-log.js against a fake window.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../bridge/webview-log.js", import.meta.url), "utf8");

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
    ["plugin:log|log", 5, "console.error: boom 3"],
    ["plugin:log|log", 4, "console.warn: careful"],
  ]);
  assert.equal(original.length, 2);
});

test("objects are written by their failure fields only", () => {
  const { win, calls } = setup();
  win.console.error({ code: "voice.failure.model", transcript: "private words" });
  assert.equal(calls[0].message, "console.error: {code=voice.failure.model}");
  win.console.error({ transcript: "private words" });
  assert.equal(calls[1].message, "console.error: [object]");
});

test("credential-shaped text is masked", () => {
  const { win, calls } = setup();
  win.console.error("failed with sk-abcdefghijkl and Bearer abcdefgh12345 api_key=hunter22hunter");
  assert.ok(!/abcdefghijkl|abcdefgh12345|hunter22/.test(calls[0].message), calls[0].message);
});

test("uncaught errors and unhandled rejections are logged", () => {
  const { listeners, calls } = setup();
  listeners.error({ error: new TypeError("bad"), filename: "http://x/app.js", lineno: 1, colno: 2 });
  listeners.unhandledrejection({ reason: { key: "voice.failure.start" } });
  assert.match(calls[0].message, /^uncaught: TypeError: bad/);
  assert.equal(calls[1].message, "unhandled rejection: {key=voice.failure.start}");
});

test("a flood is cut and long lines are shortened", () => {
  const { win, calls } = setup();
  win.console.error("x".repeat(5000));
  assert.ok(calls[0].message.length < 1100);
  for (let i = 0; i < 100; i++) win.console.error("again");
  assert.equal(calls.length, 50);
});
