// Unit tests of the mobile app's page script (mobile/tauri/js/mobile-bridge.js with the members and the transport it is composed
// of, as mobile/tauri/src/bridge.rs composes it) against a fake window: no browser, no Tauri.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { members, transport } from "./bridge-source.mjs";

const mobile = readFileSync(new URL("../mobile/tauri/js/mobile-bridge.js", import.meta.url), "utf8");
const composed = `(function (transportOf, membersOf) {\n${mobile}\n})(${transport}, ${members});`;
const PLUGIN = "plugin:sidevoice-shared-tauri|";

const live = { debug: null, plugin: {}, navigator: undefined };

function install(origin, pageOrigin = origin, microphoneSettings = true, microphoneHelp = null, messageTables = null, deviceName = null, build = { channel: "dev", version: "0.1.0" }, debugState = null) {
  const module = { exports: {} };
  vm.runInNewContext(composed, { module, Promise, JSON, Date });
  const invoked = [];
  const intervals = [];
  const url = new URL(pageOrigin);
  const win = {
    location: { protocol: url.protocol, host: url.host },
    performance: { timeOrigin: 1 },
    navigator: live.navigator,
    __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      unregisterCallback: () => {},
      invoke: (command, args) => (invoked.push([command, args]),
        command in live.plugin ? Promise.resolve().then(() => live.plugin[command](args)) : Promise.resolve(command.endsWith("voice_events") ? 1 : command === "call_page" ? 7 : command === "developer_options" && args.action === "get" ? live.debug : null)),
    },
    setInterval: (fn) => (intervals.push(fn), 1),
    clearInterval: () => {},
    setTimeout: (fn) => setTimeout(fn, 0),
    heard: new Map(),
    addEventListener(type, fn) { (this.heard.get(type) ?? this.heard.set(type, new Set()).get(type)).add(fn); },
    removeEventListener(type, fn) { this.heard.get(type)?.delete(fn); },
  };
  const api = module.exports(win, origin, microphoneSettings, microphoneHelp, messageTables, deviceName, build, debugState);
  return { api, win, invoked, intervals };
}

for (const origin of ["tauri://localhost", "http://tauri.localhost"]) {
  test(`${origin}: the page gets the members a phone offers, and owns no media`, () => {
    const { api, win } = install(origin);
    assert.equal(win.__sidevoiceHost, api);
    assert.equal(win.__SIDEVOICE_TARGET__, null);
    assert.equal(api.version, 2);
    assert.equal(typeof api.snapshot, "function");
    assert.equal(typeof api.run, "function");
    const host = api.host;
    assert.deepEqual(Object.keys(host).sort(), ["audioRoute", "build", "callSurface", "codeScanner", "cue", "engine", "microphone", "pairings", "presence", "reach", "screen", "shortcuts", "voice"]);
    assert.equal(host.callSurface.active(), true, "the app draws the call's surfaces");
    for (const fn of ["still", "forget"]) assert.equal(typeof host.callSurface[fn], "function", fn);
    assert.equal(JSON.stringify(host.presence), JSON.stringify({ media: "app" }));
    assert.equal(JSON.stringify(host.reach), JSON.stringify({ plainHttpPrivate: true }));
    for (const fn of ["catalogs", "install", "installed", "capabilities", "memory"]) assert.equal(typeof host.engine[fn], "function", fn);
    for (const fn of ["read", "write"]) assert.equal(typeof host.pairings[fn], "function", fn);
    assert.equal(typeof host.voice, "object");
    assert.ok(Object.isFrozen(host));
  });
}

test("the system's output picker is offered only where the app says it can open one, and opens it on the page's ask", async () => {
  const settled = () => new Promise((resolve) => setTimeout(resolve, 0));
  const none = install("tauri://localhost");
  await settled();
  assert.equal(JSON.stringify(none.invoked.find(([command]) => command === "audio_route")[1]), JSON.stringify({ action: "available" }));
  assert.equal(none.api.host.audioRoute, null, "no picker: the page offers none");

  let output = "AirPods de Rubén";
  live.plugin.audio_route = (args) => (args.action === "current" ? output : true);
  try {
    const { api, win, invoked } = install("tauri://localhost");
    assert.equal(api.host.audioRoute, null, "until the app answers");
    await settled();
    const { audioRoute } = api.host;
    assert.deepEqual(Object.keys(audioRoute), ["open", "current", "subscribe"]);
    assert.equal(await audioRoute.open(), undefined);
    assert.equal(await audioRoute.current(), "AirPods de Rubén", "the output's name, as the system gives it");
    const told = [];
    const stop = audioRoute.subscribe((name) => told.push(name));
    output = "iPhone";
    for (const fn of win.heard.get("sidevoice-audio-route")) fn();
    await settled();
    assert.deepEqual(told, ["iPhone"], "a change of the route tells the new name");
    stop();
    assert.equal(win.heard.get("sidevoice-audio-route").size, 0, "unsubscribed");
    output = null;
    assert.equal(await audioRoute.current(), null, "no name: null, the page keeps its own label");
    assert.equal(JSON.stringify(invoked.filter(([command]) => command === "audio_route").map(([, args]) => args)),
      JSON.stringify([{ action: "available" }, { action: "open" }, { action: "current" }, { action: "current" }, { action: "current" }]));
  } finally { delete live.plugin.audio_route; }

  live.plugin.audio_route = (args) => { if (args.action === "available") return true; throw { key: "audio_route", code: "audio-route-picker-failed" }; };
  try {
    const { api } = install("tauri://localhost");
    await settled();
    await assert.rejects(api.host.audioRoute.open(), (error) => error.code === "audio-route-picker-failed", "a refusal reaches the page");
  } finally { delete live.plugin.audio_route; }
});

test("the phone's own cue is offered only where the app says it has a player, and plays the sound the page chose", async () => {
  const settled = () => new Promise((resolve) => setTimeout(resolve, 0));
  const none = install("tauri://localhost");
  await settled();
  assert.equal(none.invoked.filter(([command]) => command === "cue").length, 1, "asked once");
  assert.equal(JSON.stringify(none.invoked.find(([command]) => command === "cue")[1]), JSON.stringify({ action: "available" }));
  assert.equal(none.api.host.cue, null, "no player: the page plays its cues itself");

  live.plugin.cue = (args) => (args.action === "available" ? true : null);
  try {
    const { api, invoked } = install("tauri://localhost");
    assert.equal(api.host.cue, null, "until the app answers");
    await settled();
    const { cue } = api.host;
    assert.equal(typeof cue.start, "function");
    assert.equal(typeof cue.stop, "function");
    // It says it plays the chosen sound at the offset and the frequency, and only while the screen is locked when asked: a page
    // then asks it for that, not for a sound of its own, and leaves the lock to it.
    assert.deepEqual([...cue.options], ["sound", "gainDb", "intervalScale", "whileLocked"]);
    assert.ok(Object.isFrozen(cue.options));
    // An older page's start, then a page's with its choice, then one with options that are not what they say, then one with the
    // frequency and for the lock.
    cue.start("thinking");
    cue.start("thinking", { sound: "deep-heartbeat", gainDb: -4 });
    cue.start("thinking", { sound: 3, gainDb: Number.NaN, extra: "x", intervalScale: "2", whileLocked: "yes" });
    cue.start("thinking", { sound: "deep-heartbeat", gainDb: -4, intervalScale: 1.5, whileLocked: true });
    cue.start("thinking", { sound: "deep-heartbeat", gainDb: -4, intervalScale: Number.POSITIVE_INFINITY });
    cue.stop("thinking");
    await settled();
    assert.equal(JSON.stringify(invoked.filter(([command]) => command === "cue").map(([, args]) => args)),
      JSON.stringify([{ action: "available" }, { action: "start", name: "thinking" }, { action: "start", name: "thinking", sound: "deep-heartbeat", gainDb: -4 },
        { action: "start", name: "thinking" }, { action: "start", name: "thinking", sound: "deep-heartbeat", gainDb: -4, intervalScale: 1.5, whileLocked: true },
        { action: "start", name: "thinking", sound: "deep-heartbeat", gainDb: -4 }, { action: "stop", name: "thinking" }]));
  } finally { delete live.plugin.cue; }

  live.plugin.cue = () => { throw new Error("no audio"); };
  try {
    const { api } = install("tauri://localhost");
    await settled();
    assert.equal(api.host.cue, null, "an app that fails to answer offers none");
  } finally { delete live.plugin.cue; }
});

test("host.screen says whether the screen is locked, through screen_locked, and only a true is locked", async () => {
  let answer = true;
  live.plugin.screen_locked = () => answer;
  try {
    const { api, invoked } = install("tauri://localhost");
    const { screen } = api.host;
    assert.deepEqual(Object.keys(screen), ["locked"]);
    assert.ok(Object.isFrozen(screen));
    assert.equal(await screen.locked(), true);
    for (const other of [false, null, "true", 1]) {
      answer = other;
      assert.equal(await screen.locked(), false, String(other));
    }
    assert.ok(invoked.filter(([command]) => command === "screen_locked").every(([, args]) => !args || Object.keys(args).length === 0), "asks nothing more");
    // An app that cannot answer: the page's question fails, and the page counts the screen as not locked.
    live.plugin.screen_locked = () => { throw new Error("no answer"); };
    await assert.rejects(screen.locked());
  } finally { delete live.plugin.screen_locked; }
});

test("the phone's wording of the web's texts is offered per language, and only when it has any", () => {
  const tables = { en: { "devices.speakerFixed": "Change it from Control Center." }, es: { "devices.speakerFixed": "Cámbialo en el Centro de control." } };
  const { messages } = install("tauri://localhost", "tauri://localhost", true, null, tables).api.host;
  assert.equal(messages("es")["devices.speakerFixed"], "Cámbialo en el Centro de control.");
  assert.equal(messages("en")["devices.speakerFixed"], "Change it from Control Center.");
  assert.deepEqual(Object.keys(messages("de")), []);
  assert.equal(install("tauri://localhost").api.host.messages, undefined);
});

test("on any other origin it does nothing", () => {
  const { api, win } = install("tauri://localhost", "https://example.com");
  assert.equal(api, null);
  assert.equal(win.__sidevoiceHost, undefined);
});

test("the shared commands go to the plugin", async () => {
  const { api, invoked } = install("tauri://localhost");
  await api.host.pairings.read();
  await api.host.engine.installed();
  const plugin = invoked.map(([command]) => command).filter((command) => command.startsWith(PLUGIN));
  assert.deepEqual(plugin, [`${PLUGIN}pairings_read`, `${PLUGIN}engine_installed`]);
});

test("the microphone's settings page is offered only where the system has one", () => {
  const withPage = install("tauri://localhost", "tauri://localhost", true).api.host.microphone;
  const without = install("tauri://localhost", "tauri://localhost", false).api.host.microphone;
  assert.equal(typeof withPage.openSettings, "function");
  assert.equal(without.openSettings, undefined);
});

test("the call is reported to the app, and the app's commands run on the room", async () => {
  const { api, win, invoked } = install("tauri://localhost");
  const settled = () => new Promise((resolve) => setTimeout(resolve, 0));
  await settled();
  // Before the room is up: reported once, not ready, with the page's number (asked once).
  assert.deepEqual(invoked.map(([command]) => command).filter((command) => command !== "cue" && command !== "audio_route"), ["call_page", "call_state"]);
  const [report] = invoked.filter(([command]) => command === "call_state");
  assert.equal(report[1].page, 7);
  assert.equal(report[1].snapshot.ready, false);
  let toggled = 0;
  win.sidevoiceActions = { toggleMic: () => { toggled++; } };
  win.sidevoiceUI = { store: { getState: () => ({ call: { joined: true }, mic: { enabled: true }, callCard: { title: "Repo" } }), subscribe: () => () => {} } };
  assert.equal(api.run({ command: "toggle-mute" }), true);
  assert.equal(toggled, 1);
  await settled();
  const last = invoked.at(-1);
  assert.equal(last[0], "call_state");
  assert.equal(last[1].page, 7);
  assert.equal(invoked.filter(([command]) => command === "call_page").length, 1, "the number is asked once");
  assert.equal(JSON.stringify([last[1].snapshot.ready, last[1].snapshot.joined, last[1].snapshot.micEnabled, last[1].snapshot.title]),
    JSON.stringify([true, true, true, "Repo"]));
});

test("the microphone is the app's, with its own words for a blocked one (iOS: AVFoundation's permission)", async () => {
  const help = { en: { steps: ["Open Sidevoice’s settings with the button below and turn on Microphone. Then check again."], openSettings: "Open Settings" },
    es: { steps: ["Abre los ajustes de Sidevoice con el botón de abajo y activa Micrófono. Después, vuelve a comprobarlo."], openSettings: "Abrir Ajustes" } };
  const { api, invoked } = install("tauri://localhost", "tauri://localhost", true, help);
  const microphone = api.host.microphone;
  // The state is the app's command, never the webview's getUserMedia.
  await microphone.state();
  assert.ok(invoked.some(([command, args]) => command === `${PLUGIN}microphone` && args.action === "state"));
  assert.equal(microphone.help("es-ES").openSettings, "Abrir Ajustes");
  assert.equal(microphone.help("de").steps[0], help.en.steps[0]);
  assert.equal(install("tauri://localhost").api.host.microphone.help, undefined, "no words, no help");
});

test("a command for a given page's call runs only on that call", async () => {
  const { api, win } = install("tauri://localhost");
  await new Promise((resolve) => setTimeout(resolve, 0)); // the page's number (7) is known
  let hungUp = 0;
  win.sidevoiceActions = { toggleCall: () => { hungUp++; } };
  win.sidevoiceUI = { store: { getState: () => ({ call: { joined: true }, mic: { enabled: true }, callCard: {} }), subscribe: () => () => {} } };
  const since = api.snapshot().since;
  assert.equal(api.run({ command: "hang-up", page: 6, since }), false, "another page's");
  assert.equal(api.run({ command: "hang-up", page: 7, since: since - 1 }), false, "another call of this page");
  assert.equal(hungUp, 0);
  assert.equal(api.run({ command: "hang-up", page: 7, since }), true);
  assert.equal(hungUp, 1);
});

/** A room with a store the test can change, as the web UI's: `change()` ticks the subscribers. */
async function room() {
  const { api, win, intervals } = install("tauri://localhost");
  await new Promise((resolve) => setTimeout(resolve, 0)); // the page's number (7) is known
  const view = { call: { joined: true }, mic: { enabled: true, disabled: false }, callCard: {} };
  const subscribers = [];
  let toggles = 0;
  // The web UI's store notifies as the action is made, not afterwards.
  win.sidevoiceActions = { toggleMic: () => { toggles++; view.mic.enabled = !view.mic.enabled; subscribers.forEach((fn) => fn()); } };
  win.sidevoiceUI = { store: { getState: () => view, subscribe: (fn) => (subscribers.push(fn), () => {}) } };
  intervals.forEach((tick) => tick()); // the room is up: the app follows its store
  const change = () => subscribers.forEach((fn) => fn());
  const since = () => api.snapshot().since;
  return {
    api, view, change, toggles: () => toggles,
    interrupt: () => api.run({ command: "interrupt", page: 7, since: since() }),
    resume: (restoreMic = true) => api.run({ command: "resume", page: 7, since: since(), restoreMic }),
  };
}

test("the system taking the audio away mutes the call and gives the mute back as it was", async () => {
  const r = await room();
  assert.equal(r.interrupt(), true);
  assert.equal(r.view.mic.enabled, false);
  r.change();
  assert.equal(r.resume(), true);
  assert.equal(r.view.mic.enabled, true);
  assert.equal(r.toggles(), 2);
});

test("a person who had muted themselves stays muted", async () => {
  const r = await room();
  r.view.mic.enabled = false;
  assert.equal(r.interrupt(), true);
  r.change();
  assert.equal(r.resume(), true);
  assert.equal(r.view.mic.enabled, false);
  assert.equal(r.toggles(), 0);
});

test("a person who takes the mute over during the interruption keeps it, whatever they do", async () => {
  const r = await room();
  r.interrupt(); // seen muted as it is made
  r.view.mic.enabled = true; // the person unmutes
  r.change();
  r.view.mic.enabled = false; // and mutes again
  r.change();
  assert.equal(r.resume(), true);
  assert.equal(r.view.mic.enabled, false, "their latest mute is not undone");
});

test("the system advising against resuming leaves the mute, and it is the person's from then on", async () => {
  const r = await room();
  r.interrupt();
  r.change();
  assert.equal(r.resume(false), true);
  assert.equal(r.view.mic.enabled, false);
  assert.equal(r.resume(), true, "a later resume changes nothing");
  assert.equal(r.view.mic.enabled, false);
});

test("a call that ends while interrupted gives the mute back for the next one", async () => {
  const r = await room();
  r.interrupt();
  r.change();
  r.view.call.joined = false; // hung up
  r.change();
  assert.equal(r.view.mic.enabled, true);
});

test("an interruption command is for one call of one page", async () => {
  const r = await room();
  assert.equal(r.api.run({ command: "interrupt", page: 6, since: r.api.snapshot().since }), false, "another page's");
  assert.equal(r.api.run({ command: "interrupt", page: 7, since: r.api.snapshot().since - 1 }), false, "another call's");
  assert.equal(r.view.mic.enabled, true);
});

test("the system's call muting or unmuting sets the page's mute, once", async () => {
  const r = await room();
  const mute = (muted, since = r.api.snapshot().since) => r.api.run({ command: "mute", page: 7, since, muted });
  assert.equal(mute(true), true);
  assert.equal(r.view.mic.enabled, false);
  assert.equal(mute(true), true, "what it is already: nothing to do");
  assert.equal(r.toggles(), 1);
  assert.equal(mute(false), true);
  assert.equal(r.view.mic.enabled, true);
  assert.equal(r.toggles(), 2);
  assert.equal(mute(true, r.api.snapshot().since - 1), false, "another call's");
  assert.equal(r.api.run({ command: "mute", page: 7, since: r.api.snapshot().since }), false, "no state asked for");
  r.view.mic.disabled = true;
  assert.equal(mute(true), false, "a mute that cannot be used");
  assert.equal(r.view.mic.enabled, true);
});

test("an unmute from the system's call takes over the interruption's mute", async () => {
  const r = await room();
  r.interrupt();
  r.change();
  r.api.run({ command: "mute", page: 7, since: r.api.snapshot().since, muted: false });
  r.api.run({ command: "mute", page: 7, since: r.api.snapshot().since, muted: true });
  assert.equal(r.resume(), true);
  assert.equal(r.view.mic.enabled, false, "the person's latest mute is not undone");
});

test("the phone's name is offered as host.deviceName, and left out when the system has none", () => {
  const named = install("tauri://localhost", undefined, true, null, null, "Galaxy de Ana");
  assert.equal(named.api.host.deviceName, "Galaxy de Ana");
  for (const none of [null, ""]) {
    const { api } = install("tauri://localhost", undefined, true, null, null, none);
    assert.equal("deviceName" in api.host, false);
  }
});

test("host.build is what the app says this build is, frozen", () => {
  const nightly = { channel: "nightly", version: "0.1.0", commit: "a1b2c3d", date: "2026-11-02" };
  const { api } = install("tauri://localhost", undefined, true, null, null, null, nightly);
  assert.deepEqual({ ...api.host.build }, nightly);
  assert.equal(Object.isFrozen(api.host.build), true);
});

test("the call's surface rides in the report, its stills by id; their bytes go before, and are forgotten", async () => {
  const { api, win, invoked, intervals } = install("tauri://localhost");
  const settled = () => new Promise((resolve) => setTimeout(resolve, 0));
  await settled();
  const surface = {
    pendingCancellable: true, model: "Opus 5.5", machine: "mac-studio", workingSince: 1700000000000, conversationCount: 2,
    still: { id: "t1#240", k: 1.3 }, others: [{ threadId: "t2", title: "B", reach: "listening", unread: 0, still: { id: "t2#108", k: 1 } }],
    words: { state: "Trabajando", offline: "Desconectada" },
  };
  const view = { call: { joined: true }, mic: { enabled: true }, callCard: { title: "Repo", mood: "working" }, callSurface: null };
  const subscribers = [];
  win.sidevoiceActions = { toggleMic: () => {} };
  win.sidevoiceUI = { store: { getState: () => view, subscribe: (fn) => (subscribers.push(fn), () => {}) } };
  intervals.forEach((tick) => tick());
  await settled();
  // The page hands the bytes over first, then publishes the snapshot that names them.
  api.host.callSurface.still({ id: "t1#240", png: "iVBORw0KGgo=", px: 240 });
  api.host.callSurface.still({ id: "t2#108", png: "iVBORw0KGgoB", px: 108 });
  view.callSurface = surface;
  subscribers.forEach((fn) => fn());
  await settled();
  const sent = invoked.filter(([command]) => ["call_still", "call_state", "call_forget"].includes(command));
  const order = sent.slice(-3).map(([command, args]) => command === "call_still" ? args.id : command);
  assert.deepEqual(order, ["t1#240", "t2#108", "call_state"], "the stills before the report that names them");
  const still = sent.find(([command]) => command === "call_still")[1];
  assert.equal(JSON.stringify(still), JSON.stringify({ page: 7, id: "t1#240", png: "iVBORw0KGgo=", px: 240 }));
  const report = sent.at(-1)[1].snapshot;
  assert.equal(report.mood, "working");
  assert.equal(JSON.stringify(report.surface), JSON.stringify(surface));
  api.host.callSurface.forget(["t2#108", 3, ""]);
  api.host.callSurface.forget([]);
  await settled();
  const forgets = invoked.filter(([command]) => command === "call_forget").map(([, args]) => args);
  assert.equal(JSON.stringify(forgets), JSON.stringify([{ page: 7, ids: ["t2#108"] }]));
});

test("a still without its id or bytes is not sent", async () => {
  const { api, invoked } = install("tauri://localhost");
  api.host.callSurface.still({ id: "", png: "x", px: 1 });
  api.host.callSurface.still({ id: "a", px: 1 });
  api.host.callSurface.still({ id: "a", png: "x" });
  api.host.callSurface.still({ id: "a", png: "x", px: 0 });
  api.host.callSurface.still();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(invoked.filter(([command]) => command === "call_still").length, 0);
});

test("a page without a call surface reports none", async () => {
  const { api, win } = install("tauri://localhost");
  await new Promise((resolve) => setTimeout(resolve, 0));
  win.sidevoiceActions = { toggleMic: () => {} };
  win.sidevoiceUI = { store: { getState: () => ({ call: { joined: true }, mic: { enabled: true }, callCard: {} }), subscribe: () => () => {} } };
  assert.equal("surface" in api.snapshot(), false);
});

test("the stack of conversations on the call's surfaces opens the page's list of conversations", async () => {
  const { api, win } = install("tauri://localhost");
  await new Promise((resolve) => setTimeout(resolve, 0));
  let opened = 0;
  win.sidevoiceActions = { toggleMic: () => {} };
  win.sidevoiceUI = { store: { getState: () => ({ call: { joined: true }, mic: { enabled: true }, callCard: {} }), subscribe: () => () => {} } };
  assert.equal(api.run({ command: "open-conversations", page: 7 }), false, "a page without the list's seam");
  win.sidevoiceUI.openConversations = () => { opened++; };
  assert.equal(api.run({ command: "open-conversations", page: 7 }), true);
  assert.equal(opened, 1);
  assert.equal(api.run({ command: "open-conversations", page: 6 }), false, "another page's");
});

test("a report waits until the app has kept the still before it, and a refused still holds nothing up and is said", async () => {
  const { api, win, invoked, intervals } = install("tauri://localhost");
  const warned = [];
  win.console = { warn: (...args) => warned.push(args) };
  const settled = async () => { for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 0)); };
  await settled();
  const answers = [];
  const invoke = win.__TAURI_INTERNALS__.invoke;
  win.__TAURI_INTERNALS__.invoke = (command, args) => command === "call_still"
    ? (invoked.push([command, args]), new Promise((resolve, reject) => answers.push({ resolve, reject })))
    : invoke(command, args);
  const view = { call: { joined: true }, mic: { enabled: true }, callCard: {}, callSurface: null };
  const subscribers = [];
  win.sidevoiceActions = { toggleMic: () => {} };
  win.sidevoiceUI = { store: { getState: () => view, subscribe: (fn) => (subscribers.push(fn), () => {}) } };
  intervals.forEach((tick) => tick());
  await settled();
  const reports = () => invoked.filter(([command]) => command === "call_state").length;
  const before = reports();
  api.host.callSurface.still({ id: "t1#240", png: "iVBORw0KGgo=", px: 240 });
  view.callSurface = { still: { id: "t1#240", k: 1 } };
  subscribers.forEach((fn) => fn());
  await settled();
  assert.equal(reports(), before, "the report names a still the app has not kept yet: it waits");
  answers[0].resolve(null);
  await settled();
  assert.equal(reports(), before + 1);
  api.host.callSurface.still({ id: "t2#108", png: "iVBORw0KGgoB", px: 108 });
  view.callSurface = { still: { id: "t2#108", k: 1 } };
  subscribers.forEach((fn) => fn());
  await settled();
  assert.equal(warned.length, 0, "a kept still says nothing");
  answers[1].reject("call_still: not a PNG of a still's size");
  await settled();
  assert.equal(reports(), before + 2, "a refusal does not stop the reports");
  // In the console the app's log takes (shared/tauri/js/webview-log.js), with the prefix its unified log makes public.
  assert.equal(JSON.stringify(warned), JSON.stringify([["[sidevoice] call surface: call_still t2#108 failed:", "call_still: not a PNG of a still's size"]]));
});

test("host.debug is the phone's one developer switch, only where the app offers it, and sets go to the app", async () => {
  const none = install("tauri://localhost");
  assert.equal("debug" in none.api.host, false, "a release has no host.debug");
  const { api, invoked } = install("tauri://localhost", undefined, true, null, null, null, undefined, { available: ["systemCall"], systemCall: false });
  const debug = api.host.debug;
  assert.deepEqual([...debug.available], ["systemCall"]);
  assert.equal(debug.systemCall, false, "off by default");
  await assert.rejects(debug.set("inspectable", true), { key: "bad_request" });
  await assert.rejects(debug.set("systemCall", "yes"), { key: "bad_request" });
  assert.equal(invoked.some(([command, args]) => command === "developer_options" && args.action === "set"), false);
  await debug.set("systemCall", true);
  assert.equal(JSON.stringify(invoked.filter(([command, args]) => command === "developer_options" && args.action === "set")), JSON.stringify([["developer_options", { action: "set", name: "systemCall", enabled: true }]]));
});

test("host.debug after a reload reads the app's live state, not the snapshot the script was built with", async () => {
  // The script's text is fixed at the app's start (systemCall off); the person turned it on since, and reloads the page.
  live.debug = { available: ["systemCall"], systemCall: true };
  try {
    const { api, invoked } = install("tauri://localhost", undefined, true, null, null, null, undefined, { available: ["systemCall"], systemCall: false });
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(JSON.stringify(invoked.filter(([command]) => command === "developer_options")), JSON.stringify([["developer_options", { action: "get" }]]));
    assert.equal(api.host.debug.systemCall, true);
  } finally {
    live.debug = null;
  }
  const release = install("tauri://localhost");
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(release.invoked.some(([command]) => command === "developer_options"), false, "a release never asks");
});

// ----- The headset's presses (host.shortcuts: the only shortcuts a phone has) -----
const HEADSET_STATUS = { shortcuts: {}, permission: null, inApp: false, headset: { press: "mute", doublePress: "none", outsideCall: "leave" } };

test("host.shortcuts reports the headset alone, asks the app, and tells whoever listens what the page set", async () => {
  const asked = [];
  live.plugin = {
    global_shortcuts: (args) => {
      asked.push(args);
      return args.action === "get"
        ? HEADSET_STATUS
        : { ...HEADSET_STATUS, headset: { ...HEADSET_STATUS.headset, ...args.headset } };
    },
  };
  try {
    const { api } = install("tauri://localhost");
    const shortcuts = api.host.shortcuts;
    for (const member of ["get", "setHeadset", "subscribe"]) assert.equal(typeof shortcuts[member], "function", member);
    for (const member of ["set", "restore", "request", "record"]) assert.equal(member in shortcuts, false, `${member}: a phone has no key to set`);
    const status = await shortcuts.get();
    assert.deepEqual(Object.keys(status.shortcuts), [], "no global shortcut is offered");
    assert.equal(status.permission, null);
    assert.equal(status.inApp, false, "the page has no keys of its own to offer");
    assert.equal(status.headset.press, "mute");
    assert.equal(status.headset.doublePress, "none");
    const heard = [];
    const stop = shortcuts.subscribe((value) => heard.push(value.headset.doublePress));
    const next = await shortcuts.setHeadset({ doublePress: "oneTurn" });
    assert.equal(next.headset.doublePress, "oneTurn");
    assert.deepEqual(heard, ["oneTurn"]);
    assert.equal(JSON.stringify(asked.at(-1)), JSON.stringify({ action: "headset", headset: { doublePress: "oneTurn" } }));
    stop();
    await shortcuts.setHeadset({ press: "skip" });
    assert.deepEqual(heard, ["oneTurn"], "a listener that stopped hears no more");
    await assert.rejects(shortcuts.setHeadset(null), { key: "bad_request" });
    await assert.rejects(shortcuts.setHeadset("mute"), { key: "bad_request" });
    assert.equal(asked.filter((args) => args.action === "headset").length, 2, "a bad change never reaches the app");
  } finally {
    live.plugin = {};
  }
});

test("the developer option that turns the system's call on is read again by whoever listens to host.shortcuts", async () => {
  let managed = false;
  live.plugin = {
    global_shortcuts: () => ({ ...HEADSET_STATUS, headset: { ...HEADSET_STATUS.headset, ...(managed ? { managedBySystem: true } : {}) } }),
    developer_options: (args) => (managed = args.enabled, { available: ["systemCall"], systemCall: managed }),
  };
  try {
    const { api } = install("tauri://localhost", undefined, true, null, null, null, undefined, { available: ["systemCall"], systemCall: false });
    const heard = [];
    api.host.shortcuts.subscribe((value) => heard.push(!!value.headset.managedBySystem));
    await api.host.debug.set("systemCall", true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(heard, [true]);
  } finally {
    live.plugin = {};
  }
});

test("a headset's press made 'say one thing' runs the room's one-turn, and does nothing where the page has none", async () => {
  const { api, win } = install("tauri://localhost");
  await new Promise((resolve) => setTimeout(resolve, 0));
  let turns = 0;
  win.sidevoiceActions = { toggleMic: () => {}, oneTurn: () => { turns++; } };
  win.sidevoiceUI = { store: { getState: () => ({ call: { joined: true }, mic: { enabled: true } }), subscribe: () => () => {} } };
  assert.equal(api.run({ command: "one-turn" }), true);
  assert.equal(turns, 1);
  win.sidevoiceActions = { toggleMic: () => {} };
  assert.equal(api.run({ command: "one-turn" }), false);
});

// ----- The camera that scans the pairing code (host.codeScanner; the barcode scanner plugin behind it) -----
const SCANNER = "plugin:barcode-scanner|";
const CODE = "SV1.eyJ2IjoxfQ";
function scanner(plugin) {
  live.plugin = plugin;
  const { api, invoked } = install("tauri://localhost");
  return { scanner: api.host.codeScanner, invoked };
}
// The members live in another realm (vm): compare their values, not their prototypes.
const plain = (value) => JSON.parse(JSON.stringify(value));
const calls = (invoked) => invoked.filter(([command]) => command.startsWith(SCANNER)).map(([command]) => command.slice(SCANNER.length));

test("a granted camera scans a QR and hands back the text as the machine printed it", async () => {
  const { scanner: camera, invoked } = scanner({
    [`${SCANNER}request_permissions`]: () => ({ camera: "granted" }),
    [`${SCANNER}scan`]: () => ({ content: CODE, format: "QR_CODE", bounds: "0,0,1,1" }),
  });
  assert.deepEqual(plain(await camera.scan()), { status: "scanned", text: CODE });
  assert.deepEqual(calls(invoked), ["request_permissions", "scan"]);
  const scan = invoked.find(([command]) => command === `${SCANNER}scan`)[1];
  assert.deepEqual(plain(scan), { windowed: true, formats: ["QR_CODE"] }, "only QR codes, seen through the page");
});

test("a denied camera is said as denied, never scans, and its settings page opens", async () => {
  const { scanner: camera, invoked } = scanner({
    [`${SCANNER}request_permissions`]: () => ({ camera: "denied" }),
    [`${SCANNER}check_permissions`]: () => ({ camera: "denied" }),
    [`${SCANNER}scan`]: () => { throw new Error("must not scan"); },
  });
  assert.equal(await camera.request(), "denied");
  assert.deepEqual(plain(await camera.scan()), { status: "denied" });
  assert.ok(!calls(invoked).includes("scan"));
  await camera.openSettings();
  assert.ok(calls(invoked).includes("open_app_settings"));
});

test("a camera the system took away between the answer and the scan is denied, not unavailable", async () => {
  const { scanner: camera } = scanner({
    [`${SCANNER}request_permissions`]: () => ({ camera: "granted" }),
    [`${SCANNER}check_permissions`]: () => ({ camera: "denied" }),
    [`${SCANNER}scan`]: () => Promise.reject("Camera permission denied or not yet requested"),
  });
  assert.deepEqual(plain(await camera.scan()), { status: "denied" });
});

test("a device without a camera is unavailable, and a plugin that is not there is denied", async () => {
  const { scanner: camera } = scanner({
    [`${SCANNER}request_permissions`]: () => ({ camera: "granted" }),
    [`${SCANNER}check_permissions`]: () => ({ camera: "granted" }),
    [`${SCANNER}scan`]: () => Promise.reject("No camera available on this device (e.g., iOS Simulator)"),
  });
  assert.deepEqual(plain(await camera.scan()), { status: "unavailable" });
  const { scanner: missing } = scanner({
    [`${SCANNER}request_permissions`]: () => Promise.reject(new Error("not allowed")),
    [`${SCANNER}check_permissions`]: () => Promise.reject(new Error("not allowed")),
  });
  assert.equal(await missing.permission(), "denied");
  assert.deepEqual(plain(await missing.scan()), { status: "denied" });
});

test("cancel ends the scan in progress as cancelled, and does nothing when there is none", async () => {
  let reject;
  const { scanner: camera, invoked } = scanner({
    [`${SCANNER}request_permissions`]: () => ({ camera: "granted" }),
    [`${SCANNER}scan`]: () => new Promise((_resolve, fail) => { reject = fail; }),
    [`${SCANNER}cancel`]: () => reject("cancelled"),
  });
  await camera.cancel();
  assert.ok(!calls(invoked).includes("cancel"), "nothing to cancel");
  const first = camera.scan();
  const second = camera.scan();
  assert.equal(first, second, "one scan at a time");
  await new Promise((resolve) => setTimeout(resolve, 5));
  await camera.cancel();
  assert.deepEqual(plain(await first), { status: "cancelled" });
  assert.equal(calls(invoked).filter((command) => command === "scan").length, 1);
});

test("a camera not yet decided is reported as prompt, and a scan after a cancel starts a new one", async () => {
  const { scanner: camera } = scanner({
    [`${SCANNER}check_permissions`]: () => ({ camera: "prompt" }),
    [`${SCANNER}request_permissions`]: () => ({ camera: "granted" }),
    [`${SCANNER}scan`]: () => ({ content: CODE }),
  });
  assert.equal(await camera.permission(), "prompt");
  assert.equal((await camera.scan()).text, CODE);
  assert.equal((await camera.scan()).text, CODE);
});

test("a phone with no video input is unavailable without asking the plugin, and a list that cannot be read is left to it", async () => {
  live.navigator = { mediaDevices: { enumerateDevices: () => Promise.resolve([{ kind: "audioinput" }]) } };
  const none = scanner({ [`${SCANNER}request_permissions`]: () => ({ camera: "granted" }), [`${SCANNER}scan`]: () => { throw new Error("must not scan"); } });
  assert.deepEqual(plain(await none.scanner.scan()), { status: "unavailable" });
  live.navigator = { mediaDevices: { enumerateDevices: () => Promise.reject(new Error("no")) } };
  const unreadable = scanner({ [`${SCANNER}request_permissions`]: () => ({ camera: "granted" }), [`${SCANNER}scan`]: () => ({ content: CODE }) });
  assert.equal((await unreadable.scanner.scan()).text, CODE);
  live.navigator = undefined;
});

test("a cancel while the camera permission is asked starts no scan", async () => {
  let answer;
  const { scanner: camera, invoked } = scanner({
    [`${SCANNER}request_permissions`]: () => new Promise((resolve) => { answer = resolve; }),
    [`${SCANNER}scan`]: () => { throw new Error("must not scan"); },
    [`${SCANNER}cancel`]: () => {},
  });
  const scanning = camera.scan();
  await camera.cancel();
  answer({ camera: "granted" });
  assert.deepEqual(plain(await scanning), { status: "cancelled" });
  assert.ok(!calls(invoked).includes("scan"));
});

// ----- The phone's text size (host.textScale: Android's webview does not follow the font scale by itself) -----

test("host.textScale is the app's answer where there is one (Android), live, with the change event; none on iOS", () => {
  const listeners = new Map();
  let scale = 1;
  const { api, win } = install("tauri://localhost");
  assert.equal("textScale" in api.host, false, "no answer from the app: nothing offered");
  const withNative = (() => {
    const module = { exports: {} };
    vm.runInNewContext(composed, { module, Promise, JSON, Date });
    const url = new URL("tauri://localhost");
    const fake = {
      location: { protocol: url.protocol, host: url.host }, performance: { timeOrigin: 1 }, navigator: undefined,
      SidevoiceTextScale: { get: () => scale },
      addEventListener: (type, fn) => listeners.set(type, fn), removeEventListener: (type) => listeners.delete(type),
      __TAURI_INTERNALS__: { transformCallback: () => 1, unregisterCallback: () => {}, invoke: () => Promise.resolve(null) },
      setInterval: () => 1, clearInterval: () => {}, setTimeout: (fn) => setTimeout(fn, 0),
    };
    return module.exports(fake, "tauri://localhost", true, null, null, null, { channel: "dev", version: "0.1.0" }, null);
  })();
  assert.equal(win.__sidevoiceHost.host.textScale, undefined);
  const ts = withNative.host.textScale;
  assert.equal(ts.get(), 1);
  scale = 1.3;
  assert.equal(ts.get(), 1.3, "read live");
  const heard = [];
  const stop = ts.subscribe((value) => heard.push(value));
  listeners.get("sidevoice-text-scale")();
  assert.deepEqual(heard, [1.3]);
  stop();
  assert.equal(listeners.has("sidevoice-text-scale"), false);
});
