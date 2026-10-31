// Unit tests of the shared members (app-core/js/members.js) against a transport of the tests' own: no browser, no Tauri.
// What these tests hand the members is the whole of what the members know of how the app is reached.
import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { members as source } from "./bridge-source.mjs";

const makeMembers = vm.runInNewContext(source, { Promise, JSON, Date });
const flush = () => new Promise((resolve) => setImmediate(resolve));
const plain = (value) => (value === undefined ? undefined : JSON.parse(JSON.stringify(value))); // objects made by the members live in their own VM realm
const PAGE = 5;
const TIME_ORIGIN = 1700000000000.5; // when the fake page started: the document it says it is

/** A transport that keeps what it is asked and answers as `answer` says (the page's number for `voice_events`). */
function fakeTransport(answer = () => undefined) {
  const calls = [];
  const sent = [];
  const channels = [];
  const transport = {
    call: (command, args) => {
      calls.push([command, plain(args)]);
      return command === "voice_events" ? Promise.resolve(PAGE) : Promise.resolve().then(() => answer(command, args));
    },
    send: (command, args) => { sent.push([command, plain(args)]); },
    channel: (onmessage) => `channel-${channels.push(onmessage) - 1}`,
  };
  return { transport, calls, sent, channels };
}

function fakeWindow(extra = {}) {
  const timers = [];
  const win = {
    atob: (text) => Buffer.from(text, "base64").toString("binary"),
    performance: { timeOrigin: TIME_ORIGIN },
    setInterval: (fn) => (timers.push(fn), timers.length),
    clearInterval: (id) => { timers[id - 1] = null; },
    setTimeout: (fn, ms) => setTimeout(fn, Math.min(ms, 5)),
    ...extra,
  };
  return { win, tick: () => timers.forEach((fn) => fn && fn()) };
}

test("the members name no shell", () => {
  assert.doesNotMatch(source, /tauri|plugin:|__TAURI/i);
});

test("the engine's calls reach the app through the transport, and a refusal is coded as the page reads it", async () => {
  const { transport, calls } = fakeTransport((command) => {
    if (command === "engine_has_credential") return Promise.reject({ key: "bad_request" });
    return [];
  });
  const engine = makeMembers(fakeWindow().win, transport).engine();
  assert.deepEqual(plain(await engine.catalogs()), []);
  await engine.setCredential("openai", "sk-1");
  await assert.rejects(engine.hasCredential("openai"), { key: "bad_request", code: "bad-request" });
  assert.deepEqual(calls, [
    ["engine_catalogs", undefined],
    ["engine_set_credential", { provider: "openai", key: "sk-1" }],
    ["engine_has_credential", { provider: "openai" }],
  ]);
});

test("the voice call opens its channel through the transport once, and every command carries the page's number", async () => {
  const { transport, calls, channels } = fakeTransport();
  const voice = makeMembers(fakeWindow().win, transport).voice();
  const call = voice.host.start({ input: { model: "m" } });
  call.ready.catch(() => {});
  const states = [];
  call.onState((state) => states.push(state.listening));
  await flush();
  await flush();
  assert.equal(channels.length, 1, "one channel for the page");
  assert.deepEqual(calls[0], ["voice_events", { document: TIME_ORIGIN, events: "channel-0" }], "it says which document asks");
  assert.deepEqual(calls.map(([command]) => command).slice(0, 3), ["voice_events", "voice_create_call", "voice_start"]);
  assert.ok(calls.slice(1).every(([, args]) => args.page === PAGE), "the page's number travels with each command");
  // What the transport hands over is a message of the app, in the order it was made.
  voice.receive({ key: call.id, type: "state", data: { listening: "listening" } });
  channels[0]({ key: call.id, type: "state", data: { listening: "speaking" } });
  assert.deepEqual(states, ["listening", "speaking"]);
});

test("the microphone offers the system's settings page only where the app says the OS has one", async () => {
  const { transport, calls } = fakeTransport(() => "granted");
  const make = (settings) => makeMembers(fakeWindow().win, transport).microphone(settings);
  assert.equal(make(false).openSettings, undefined);
  assert.equal(make(undefined).openSettings, undefined);
  const microphone = make(true);
  assert.equal(await microphone.state(), "granted");
  await microphone.openSettings();
  assert.deepEqual(calls, [["microphone", { action: "state" }], ["microphone", { action: "open-settings" }]]);
});

test("the pairings are read and written whole", async () => {
  const { transport, calls } = fakeTransport();
  const pairings = makeMembers(fakeWindow().win, transport).pairings();
  await pairings.read();
  assert.equal(await pairings.write({ pairings: [] }), undefined);
  assert.deepEqual(calls, [["pairings_read", undefined], ["pairings_write", { value: { pairings: [] } }]]);
});

// --- The room's call state and its commands ---

function fakeStore(initial) {
  let state = initial;
  const listeners = new Set();
  return {
    getState: () => state,
    subscribe: (fn) => (listeners.add(fn), () => listeners.delete(fn)),
    set(next) {
      state = { ...state, ...next };
      for (const fn of listeners) fn(state);
    },
  };
}

const IN_A_CALL = { call: { joined: true, busy: false }, mic: { enabled: true, disabled: false }, callCard: { canSkip: true } };

function room(options = {}, view = IN_A_CALL) {
  const store = fakeStore(view);
  const ran = [];
  const actions = Object.fromEntries(["toggleMic", "toggleCall", "skipReply", "keepVoiceOn", "cancelInput"]
    .map((name) => [name, () => ran.push(name)]));
  const page = fakeWindow({ sidevoiceUI: { store }, sidevoiceActions: actions });
  const published = [];
  const made = makeMembers(page.win, fakeTransport().transport).room({
    version: 2,
    publish: (snapshot) => published.push(plain(snapshot)),
    extend: (view) => (view ? { extra: "of the app" } : { extra: null }),
    ...options,
  });
  return { ...page, store, ran, published, room: made };
}

test("the room reports a snapshot when the call changes, with the fields the app adds, and not when nothing did", () => {
  const { room: reporting, store, published } = room();
  reporting.start();
  assert.equal(published.length, 1);
  assert.equal(published[0].ready, true);
  assert.equal(published[0].joined, true);
  assert.equal(published[0].version, 2);
  assert.equal(published[0].extra, "of the app");
  assert.equal(reporting.joined(), true);
  store.set({ mic: { enabled: true, disabled: false } }); // the store ticks, the call did not change
  assert.equal(published.length, 1);
  store.set({ call: { joined: false } });
  assert.equal(published.length, 2);
  assert.equal(published[1].joined, false);
  assert.equal(reporting.joined(), false);
});

test("before the room is up the snapshot says so, with the app's fields of an empty call; the room is followed once it is", () => {
  const attached = [];
  const page = fakeWindow();
  const published = [];
  const reporting = makeMembers(page.win, fakeTransport().transport).room({
    version: 2,
    publish: (snapshot) => published.push(plain(snapshot)),
    extend: (view) => (view ? { extra: "room" } : { extra: "empty" }),
    attached: (ui) => attached.push(ui),
  });
  reporting.start();
  assert.deepEqual(published.map((s) => [s.ready, s.extra]), [[false, "empty"]]);
  assert.equal(reporting.run({ command: "toggle-mute" }), false, "nothing runs before the room is up");
  page.tick();
  assert.equal(attached.length, 0, "the room's seams are not there yet");
  const store = fakeStore(IN_A_CALL);
  page.win.sidevoiceUI = { store };
  page.win.sidevoiceActions = {};
  page.tick();
  page.tick();
  assert.equal(attached.length, 1, "attached once");
  assert.deepEqual(published.map((s) => [s.ready, s.extra]), [[false, "empty"], [true, "room"]]);
});

test("the shared commands run through the room's actions and answer whether they ran", () => {
  const { room: reporting, ran } = room();
  reporting.start();
  assert.equal(reporting.run({ command: "toggle-mute" }), true);
  assert.equal(reporting.run({ command: "hang-up" }), true);
  assert.equal(reporting.run({ command: "skip-reply" }), true);
  assert.equal(reporting.run({ command: "keep-alive" }), true);
  assert.equal(reporting.run({ command: "cancel-input" }), true);
  assert.deepEqual(ran, ["toggleMic", "toggleCall", "skipReply", "keepVoiceOn", "cancelInput"]);
});

test("a command is refused when the call does not allow it", () => {
  const idle = room({}, { call: { joined: false }, mic: { enabled: true, disabled: false }, callCard: {} });
  idle.room.start();
  assert.equal(idle.room.run({ command: "hang-up" }), false, "no call to hang up");
  assert.equal(idle.room.run({ command: "skip-reply" }), false, "nothing is playing");
  const disabled = room({}, { ...IN_A_CALL, mic: { enabled: true, disabled: true } });
  disabled.room.start();
  assert.equal(disabled.room.run({ command: "toggle-mute" }), false, "the room's mute button is disabled");
  assert.deepEqual(disabled.ran, []);
});

test("the commands the app adds run with the snapshot and the actions, and only the ones it named", () => {
  const seen = [];
  const { room: reporting } = room({
    commands: {
      "open-card": (request, state, actions) => (seen.push([request.where, state.joined, typeof actions.toggleMic]), true),
      "declined": () => false,
      "unsure": () => "yes",
    },
  });
  reporting.start();
  assert.equal(reporting.run({ command: "open-card", where: "corner" }), true);
  assert.deepEqual(seen, [["corner", true, "function"]]);
  assert.equal(reporting.run({ command: "declined" }), false);
  assert.equal(reporting.run({ command: "unsure" }), false, "only a true answer is a command that ran");
  // What every object has is not a command of the app.
  for (const command of ["constructor", "toString", "hasOwnProperty", "__proto__", undefined, 7]) {
    assert.equal(reporting.run({ command }), false, String(command));
  }
  assert.equal(reporting.run(null), false);
  assert.equal(reporting.run("hang-up"), false);
});
