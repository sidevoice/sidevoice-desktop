// Unit tests of bridge/call-controls-bridge.js against a fake window: no browser, no Tauri.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../bridge/call-controls-bridge.js", import.meta.url), "utf8");
const APP = "tauri://localhost";

function load() {
  const module = { exports: {} };
  vm.runInNewContext(source, { module, Promise, Object, Number });
  return module.exports;
}

function fakeWindow(origin) {
  const calls = [];
  const url = new URL(origin);
  const win = {
    location: { protocol: url.protocol, host: url.host },
    __TAURI_INTERNALS__: { invoke: (cmd, args) => (calls.push([cmd, args && JSON.parse(JSON.stringify(args))]), Promise.resolve()) },
  };
  return { win, calls };
}
const flush = () => new Promise((resolve) => setImmediate(resolve));

test("does nothing on any origin but the app's own", () => {
  const { win, calls } = fakeWindow("https://evil.example.com");
  assert.equal(load()(win, APP), null);
  assert.equal(win.__sidevoiceDesktop, undefined);
  assert.equal(calls.length, 0);
});

test("the card subscribes, asks for the state, and hears every change as a whole state", async () => {
  const { win, calls } = fakeWindow(APP);
  const api = load()(win, APP);
  const heard = [];
  const stop = win.__sidevoiceDesktop.host.callControls.subscribe((state) => heard.push(state));
  await flush();
  assert.deepEqual(calls, [["call_controls_ready", undefined]]);
  api.receive({ level: 30 });
  assert.equal(heard.length, 0, "no call yet: nothing to show");
  api.receive({ call: { joined: true }, alwaysExpanded: false });
  api.receive({ level: 50 });
  assert.deepEqual(heard.map((s) => [s.call.joined, s.level]), [[true, 30], [true, 50]]);
  stop();
  api.receive({ level: 60 });
  assert.equal(heard.length, 2);
  // A late subscriber gets the state at once, without asking.
  const late = [];
  win.__sidevoiceDesktop.host.callControls.subscribe((state) => late.push(state.level));
  assert.deepEqual(late, [60]);
});

test("commands, layout and drags reach the app, and only well-formed ones", async () => {
  const { win, calls } = fakeWindow(APP);
  load()(win, APP);
  const host = win.__sidevoiceDesktop.host.callControls;
  host.run({ command: "toggle-mute" });
  host.layout({ width: 344, height: 82 });
  host.layout({ width: 0, height: 82 });
  host.drag("start");
  host.drag("fly");
  await flush();
  assert.deepEqual(calls, [
    ["call_controls_run", { command: { command: "toggle-mute" } }],
    ["call_controls_layout", { width: 344, height: 82 }],
    ["call_controls_drag", { phase: "start" }],
  ]);
});

test("installs once, and survives a missing Tauri runtime", async () => {
  const { win } = fakeWindow(APP);
  delete win.__TAURI_INTERNALS__;
  const first = load()(win, APP);
  assert.equal(load()(win, APP), first);
  first.host.callControls.run({ command: "hang-up" });
  await flush();
});

test("the placeholder the app replaces is present exactly once", () => {
  assert.equal(source.split('"__SIDEVOICE_APP_ORIGIN__"').length, 2);
});
