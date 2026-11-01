// Unit tests of the Tauri transport (app-tauri/js/transport.js) against a fake `__TAURI_INTERNALS__`: no browser, no Tauri.
import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { transport as source } from "./bridge-source.mjs";

const makeTransport = vm.runInNewContext(source, { Promise });
const flush = () => new Promise((resolve) => setImmediate(resolve));
const PLUGIN = "plugin:sidevoice-app-tauri|";

function page() {
  const invoked = [];
  const callbacks = [];
  let answer = () => Promise.resolve("answer");
  const internals = {
    invoke: (command, args, options) => (invoked.push([command, args, options]), answer(command)),
    transformCallback: (callback) => callbacks.push(callback) - 1,
    unregisterCallback: (id) => { callbacks[id] = null; },
  };
  const win = { __TAURI_INTERNALS__: internals };
  return { win, invoked, callbacks, answer: (fn) => { answer = fn; }, internals };
}

test("the plugin's commands are invoked under its name, and the app's own under theirs", async () => {
  const { win, invoked } = page();
  const transport = makeTransport(win);
  assert.equal(await transport.call("voice_start", { key: "a" }, { headers: {} }), "answer");
  transport.send("microphone", { action: "state" });
  assert.equal(await transport.own.call("local_host_state"), "answer");
  transport.own.send("bridge_state", { snapshot: {} });
  await flush();
  assert.deepEqual(invoked.map(([command]) => command),
    [`${PLUGIN}voice_start`, `${PLUGIN}microphone`, "local_host_state", "bridge_state"]);
  assert.deepEqual(invoked[0].slice(1), [{ key: "a" }, { headers: {} }], "the arguments and options go as they are");
});

test("a page with no host rejects the call and is silently sent nothing", async () => {
  const transport = makeTransport({});
  await assert.rejects(transport.call("engine_catalogs"), { message: "no desktop host" });
  await assert.rejects(transport.own.call("data_reset_report"), { message: "no desktop host" });
  assert.equal(transport.send("bridge_level", { level: 1 }), undefined);
  assert.equal(transport.own.send("bridge_state", {}), undefined);
});

test("the host is read at each use: the app may be mid-reload", async () => {
  const win = {};
  const transport = makeTransport(win);
  await assert.rejects(transport.call("engine_memory"));
  const { internals, invoked } = page();
  win.__TAURI_INTERNALS__ = internals;
  await transport.call("engine_memory");
  assert.deepEqual(invoked.map(([command]) => command), [`${PLUGIN}engine_memory`]);
});

test("a send never fails the page, whatever the app answers", async () => {
  const { win, answer } = page();
  answer(() => Promise.reject({ key: "bad_request" }));
  const transport = makeTransport(win);
  transport.send("bridge_playback", { level: 0.5, threadId: null });
  transport.own.send("call_controls_settings", { action: "preview" });
  answer(() => { throw new Error("the host fell over"); });
  transport.send("bridge_level", { level: 3 });
  await flush();
});

test("a channel hands the app's messages over in index order, and is let go when it has ended", () => {
  const { win, callbacks } = page();
  const transport = makeTransport(win);
  const heard = [];
  const channel = transport.channel((message) => heard.push(message));
  assert.match(channel, /^__CHANNEL__:\d+$/);
  const id = Number(channel.split(":")[1]);
  callbacks[id]({ index: 2, message: "c" });
  callbacks[id]({ index: 0, message: "a" });
  assert.deepEqual(heard, ["a"], "number 1 has not come");
  callbacks[id]({ index: 1, message: "b" });
  assert.deepEqual(heard, ["a", "b", "c"]);
  callbacks[id]({ index: 0, message: "again" });
  assert.equal(heard.length, 3, "a message handed over already is not handed over again");
  callbacks[id]({ index: 3, end: true });
  assert.equal(callbacks[id], null, "the callback is unregistered once every message before the end came");
});

test("a listener that throws does not stop the messages after it", () => {
  const { win, callbacks } = page();
  const heard = [];
  const channel = makeTransport(win).channel((message) => {
    heard.push(message);
    if (message === "a") throw new Error("the page's own");
  });
  const id = Number(channel.split(":")[1]);
  callbacks[id]({ index: 0, message: "a" });
  callbacks[id]({ index: 1, message: "b" });
  assert.deepEqual(heard, ["a", "b"]);
});

test("the transport is frozen once made", () => {
  const transport = makeTransport(page().win);
  assert.ok(Object.isFrozen(transport) && Object.isFrozen(transport.own));
});
