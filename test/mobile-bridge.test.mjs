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
      invoke: (command, args) => (invoked.push([command, args]), Promise.resolve(command.endsWith("voice_events") ? 1 : null)),
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
  // Before the room is up: reported once, not ready.
  assert.deepEqual(invoked.map(([command]) => command), ["call_state"]);
  assert.equal(invoked[0][1].snapshot.ready, false);
  let toggled = 0;
  win.sidevoiceActions = { toggleMic: () => { toggled++; } };
  win.sidevoiceUI = { store: { getState: () => ({ call: { joined: true }, mic: { enabled: true }, callCard: { title: "Repo" } }), subscribe: () => () => {} } };
  assert.equal(api.run({ command: "toggle-mute" }), true);
  assert.equal(toggled, 1);
  await settled();
  const last = invoked.at(-1);
  assert.equal(last[0], "call_state");
  assert.equal(JSON.stringify([last[1].snapshot.ready, last[1].snapshot.joined, last[1].snapshot.micEnabled, last[1].snapshot.title]),
    JSON.stringify([true, true, true, "Repo"]));
});

test("the microphone is the app's, with its own words for a blocked one (iOS: AVFoundation's permission)", async () => {
  const help = { en: { steps: ["Open Settings › Sidevoice and turn on Microphone. Then check again."], openSettings: "Open Settings" },
    es: { steps: ["Abre Ajustes › Sidevoice y activa Micrófono. Después vuelve a comprobar."], openSettings: "Abrir Ajustes" } };
  const { api, invoked } = install("tauri://localhost", "tauri://localhost", true, help);
  const microphone = api.host.microphone;
  // The state is the app's command, never the webview's getUserMedia.
  await microphone.state();
  assert.ok(invoked.some(([command, args]) => command === `${PLUGIN}microphone` && args.action === "state"));
  assert.equal(microphone.help("es-ES").openSettings, "Abrir Ajustes");
  assert.equal(microphone.help("de").steps[0], help.en.steps[0]);
  assert.equal(install("tauri://localhost").api.host.microphone.help, undefined, "no words, no help");
});
