// Unit tests of the mobile app's page script (mobile/js/mobile-bridge.js with the members and the transport it is composed
// of, as mobile/src/bridge.rs composes it) against a fake window: no browser, no Tauri.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { members, transport } from "./bridge-source.mjs";

const mobile = readFileSync(new URL("../mobile/js/mobile-bridge.js", import.meta.url), "utf8");
const composed = `(function (transportOf, membersOf) {\n${mobile}\n})(${transport}, ${members});`;
const PLUGIN = "plugin:sidevoice-app-tauri|";

function install(origin, pageOrigin = origin, microphoneSettings = true, microphoneHelp = null) {
  const module = { exports: {} };
  vm.runInNewContext(composed, { module, Promise, JSON, Date });
  const invoked = [];
  const url = new URL(pageOrigin);
  const win = {
    location: { protocol: url.protocol, host: url.host },
    performance: { timeOrigin: 1 },
    __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      unregisterCallback: () => {},
      invoke: (command, args) => (invoked.push([command, args]),
        Promise.resolve(command.endsWith("voice_events") ? 1 : command === "call_page" ? 7 : null)),
    },
    setInterval: () => 1,
    clearInterval: () => {},
    setTimeout: (fn) => setTimeout(fn, 0),
  };
  const api = module.exports(win, origin, microphoneSettings, microphoneHelp);
  return { api, win, invoked };
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
    assert.deepEqual(Object.keys(host).sort(), ["engine", "microphone", "pairings", "presence", "reach", "voice"]);
    assert.equal(JSON.stringify(host.presence), JSON.stringify({ media: "app" }));
    assert.equal(JSON.stringify(host.reach), JSON.stringify({ plainHttpPrivate: true }));
    for (const fn of ["catalogs", "install", "installed", "capabilities", "memory"]) assert.equal(typeof host.engine[fn], "function", fn);
    for (const fn of ["read", "write"]) assert.equal(typeof host.pairings[fn], "function", fn);
    assert.equal(typeof host.voice, "object");
    assert.ok(Object.isFrozen(host));
  });
}

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

test("the system taking the audio away mutes the call and gives the mute back as it was", async () => {
  const { api, win } = install("tauri://localhost");
  await new Promise((resolve) => setTimeout(resolve, 0)); // the page's number (7) is known
  const mic = { enabled: true, disabled: false };
  let toggles = 0;
  win.sidevoiceActions = { toggleMic: () => { toggles++; mic.enabled = !mic.enabled; } };
  win.sidevoiceUI = { store: { getState: () => ({ call: { joined: true }, mic, callCard: {} }), subscribe: () => () => {} } };
  const interrupt = { command: "interrupt", page: 7 };
  const resume = { command: "resume", page: 7, restoreMic: true };

  // An open microphone: muted for the interruption, open again after.
  assert.equal(api.run(interrupt), true);
  assert.equal(mic.enabled, false);
  assert.equal(api.run(resume), true);
  assert.equal(mic.enabled, true);
  assert.equal(toggles, 2);

  // A person who had muted themselves stays muted.
  mic.enabled = false;
  toggles = 0;
  assert.equal(api.run(interrupt), true);
  assert.equal(api.run(resume), true);
  assert.equal(mic.enabled, false);
  assert.equal(toggles, 0);

  // A person who unmuted during the interruption is not touched.
  mic.enabled = true;
  assert.equal(api.run(interrupt), true);
  mic.enabled = true; // muted by the interruption, unmuted by the person
  assert.equal(api.run(resume), true);
  assert.equal(mic.enabled, true);

  // The system advises against resuming: the mute stays until the person lifts it, and is not remembered.
  assert.equal(api.run(interrupt), true);
  assert.equal(mic.enabled, false);
  assert.equal(api.run({ ...resume, restoreMic: false }), true);
  assert.equal(mic.enabled, false);
  mic.enabled = true;
  assert.equal(api.run(resume), true, "a later resume does not mute or unmute anyone");
  assert.equal(mic.enabled, true);

  // Another page's command does nothing.
  assert.equal(api.run({ command: "interrupt", page: 6 }), false);
});
