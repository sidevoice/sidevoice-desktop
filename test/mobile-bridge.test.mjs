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

const live = { debug: null };

function install(origin, pageOrigin = origin, microphoneSettings = true, microphoneHelp = null, messageTables = null, deviceName = null, build = { channel: "dev", version: "0.1.0" }, debugState = null) {
  const module = { exports: {} };
  vm.runInNewContext(composed, { module, Promise, JSON, Date });
  const invoked = [];
  const intervals = [];
  const url = new URL(pageOrigin);
  const win = {
    location: { protocol: url.protocol, host: url.host },
    performance: { timeOrigin: 1 },
    __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      unregisterCallback: () => {},
      invoke: (command, args) => (invoked.push([command, args]),
        Promise.resolve(command.endsWith("voice_events") ? 1 : command === "call_page" ? 7 : command === "developer_options" && args.action === "get" ? live.debug : null)),
    },
    setInterval: (fn) => (intervals.push(fn), 1),
    clearInterval: () => {},
    setTimeout: (fn) => setTimeout(fn, 0),
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
    assert.deepEqual(Object.keys(host).sort(), ["build", "engine", "microphone", "pairings", "presence", "reach", "voice"]);
    assert.equal(JSON.stringify(host.presence), JSON.stringify({ media: "app" }));
    assert.equal(JSON.stringify(host.reach), JSON.stringify({ plainHttpPrivate: true }));
    for (const fn of ["catalogs", "install", "installed", "capabilities", "memory"]) assert.equal(typeof host.engine[fn], "function", fn);
    for (const fn of ["read", "write"]) assert.equal(typeof host.pairings[fn], "function", fn);
    assert.equal(typeof host.voice, "object");
    assert.ok(Object.isFrozen(host));
  });
}

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
  assert.deepEqual(invoked.map(([command]) => command), ["call_page", "call_state"]);
  assert.equal(invoked[1][1].page, 7);
  assert.equal(invoked[1][1].snapshot.ready, false);
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
