// Unit tests of bridge/desktop-bridge.js against a fake window: no browser, no Tauri.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";

const source = readFileSync(new URL("../bridge/desktop-bridge.js", import.meta.url), "utf8");

function loadFactory() {
  const module = { exports: {} };
  vm.runInNewContext(source, { module, Promise, JSON, Date });
  return module.exports;
}

function fakeStore(initial) {
  let state = initial;
  const listeners = new Set();
  return {
    getState: () => state,
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    set(next) {
      const prev = state;
      state = { ...state, ...next };
      for (const fn of listeners) fn(state, prev);
    },
  };
}

function fakeWindow(origin) {
  const calls = [];
  const timers = [];
  const win = {
    location: { protocol: new URL(origin).protocol, host: new URL(origin).host },
    __TAURI_INTERNALS__: { invoke: (cmd, args) => (calls.push([cmd, structuredClone(args)]), Promise.resolve()) },
    setInterval: (fn) => (timers.push(fn), timers.length),
    clearInterval: (id) => (timers[id - 1] = null),
    setTimeout: (fn, ms) => setTimeout(fn, Math.min(ms, 5)),
  };
  return { win, calls, tick: () => timers.forEach((fn) => fn && fn()) };
}

const flush = () => new Promise((resolve) => setImmediate(resolve));
const plain = (value) => JSON.parse(JSON.stringify(value)); // objects made by the injected bridge live in its VM realm
const ORIGIN = "https://voice.example.com";

test("does nothing on any other origin (auth pages, other sites)", () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow("https://accounts.google.com");
  assert.equal(install(win, ORIGIN), null);
  assert.equal(win.__sidevoiceDesktop, undefined);
  assert.equal(calls.length, 0);
});

// The factory's arguments up to the microphone settings, then the uninstall offer.
const uninstallArgs = (uninstall) => [null, false, false, false, undefined, undefined, undefined, false, uninstall];

test("the data bridge uses fixed native commands and no page-supplied paths", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const api = install(win, ORIGIN, ...uninstallArgs(true));
  assert.equal(typeof api.host.data.deleteAll, "function");
  assert.equal(typeof api.host.data.resetReport, "function");
  assert.equal(typeof api.host.data.uninstall, "function");
  await api.host.data.pairings.read();
  await api.host.data.pairings.write({ pairings: [], pendingRevokes: [] });
  await api.host.data.deleteAll();
  await api.host.data.uninstall();
  await api.host.data.resetReport();
  assert.deepEqual(calls.slice(-5), [
    ["data_pairings_read", undefined],
    ["data_pairings_write", { value: { pairings: [], pendingRevokes: [] } }],
    ["data_delete_all", undefined],
    ["data_uninstall", undefined],
    ["data_reset_report", undefined],
  ]);
});

test("the reset is on every host, the uninstall only where offered, and neither follows the local host", () => {
  const install = loadFactory();
  // Windows and Linux: the reset, without the app's own removal.
  const reset = install(fakeWindow(ORIGIN).win, ORIGIN, ...uninstallArgs(false)).host.data;
  assert.equal(typeof reset.pairings.read, "function");
  assert.equal(typeof reset.deleteAll, "function");
  assert.equal(typeof reset.resetReport, "function");
  assert.equal(reset.uninstall, undefined);
  // Nothing told: the reset is still there, the uninstall is not.
  const plain = install(fakeWindow(ORIGIN).win, ORIGIN).host.data;
  assert.equal(typeof plain.deleteAll, "function");
  assert.equal(plain.uninstall, undefined);
  // The local host alone offers no uninstall.
  const localOnly = install(fakeWindow(ORIGIN).win, ORIGIN, null, true).host.data;
  assert.equal(localOnly.uninstall, undefined);
});

test("reports not-ready first, then the real state once the room's controller exists", async () => {
  const install = loadFactory();
  const { win, calls, tick } = fakeWindow(ORIGIN);
  const api = install(win, ORIGIN);
  await flush();
  assert.deepEqual(calls.map(([cmd, args]) => [cmd, args.snapshot.ready]), [["bridge_state", false]]);
  assert.equal(api.run({ command: "toggle-mute" }), false, "nothing to drive yet");

  const store = fakeStore({ call: { joined: false, busy: false }, mic: { enabled: true, disabled: false }, title: "Viewed", callCard: { title: "Claude" } });
  win.sidevoiceUI = { store };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {} };
  tick();
  await flush();
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[1][1].snapshot, {
    version: 2, ready: true, joined: false, busy: false, micEnabled: true, micDisabled: false, title: "Claude",
    agent: "idle", youTalking: false, canSkip: false, since: null, participants: [], devices: null,
  });

  store.set({ now: 123 }); // unrelated ticks are not reported
  await flush();
  assert.equal(calls.length, 2);

  store.set({ call: { joined: true, busy: false } });
  await flush();
  assert.equal(calls.length, 3);
  assert.equal(calls[2][1].snapshot.joined, true);
});

test("toggle-mute and hang-up go through the web UI's actions", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  const store = fakeStore({ call: { joined: false }, mic: { enabled: true, disabled: false } });
  let mics = 0;
  let calls = 0;
  win.sidevoiceUI = { store };
  win.sidevoiceActions = {
    toggleMic() { mics++; store.set({ mic: { enabled: !store.getState().mic.enabled, disabled: false } }); },
    toggleCall: async () => { calls++; },
  };
  const api = install(win, ORIGIN);

  assert.equal(api.run({ command: "hang-up" }), false, "no call to hang up");
  assert.equal(calls, 0);
  assert.equal(api.run({ command: "toggle-mute" }), true);
  assert.equal(mics, 1);
  assert.equal(api.snapshot().micEnabled, false);

  store.set({ call: { joined: true } });
  assert.equal(api.run({ command: "hang-up" }), true);
  assert.equal(calls, 1);

  store.set({ mic: { enabled: true, disabled: true } });
  assert.equal(api.run({ command: "toggle-mute" }), false, "disabled in the web UI, disabled here");
  assert.equal(mics, 1);
  assert.equal(api.run({ command: "rm -rf" }), false);
  assert.equal(api.run("toggle-mute"), false, "a command is an object");
});

test("the call controls card's view: the agent, you, the clock, the conversations and the devices", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const participants = [
    { threadId: "a", title: "mini", selected: true, reach: "listening", machine: "daimon", harness: "claude", working: false, subtitle: "", machineReachable: true, karaoke: "not relayed" },
    { threadId: "b", title: "web", selected: false, reach: "holding", machine: "nas", harness: "codex", working: true, subtitle: "2 sin leer" },
  ];
  // The room's facts, as its voice lists the devices (room-session-state.d.ts SessionFacts.audioDevices).
  const audioDevices = { inputs: [{ id: "CoreAudio:mbp", label: "MacBook Pro Microphone" }, { id: "CoreAudio:usb", label: "" }],
    outputs: [{ id: "CoreAudio:speakers", label: "Speakers" }], input: "CoreAudio:usb", output: "CoreAudio:speakers", outputSelectable: false };
  const store = fakeStore({ call: { joined: false }, mic: { enabled: true }, title: "a browsed transcript", participants, facts: { audioDevices },
    callCard: { agent: "idle", youTalking: false, canSkip: false, title: "mini" } });
  win.sidevoiceUI = { store };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {} };
  const api = install(win, ORIGIN);
  await flush();

  let s = api.snapshot();
  assert.equal(s.since, null, "no clock outside a call");
  const plain = (value) => JSON.parse(JSON.stringify(value)); // objects made in the bridge's own realm
  assert.deepEqual(plain(s.participants[0]), { threadId: "a", title: "mini", selected: true, reach: "listening", machine: "daimon", harness: "claude", working: false, subtitle: "", machineReachable: true },
    "whether its machine can be reached too: the card tells an agent away from a machine away by it (sidevoice-web#356)");
  // As the card reads them (call-controls/host.ts AudioDevices): the system's default first, an unnamed device by its
  // place, and a speaker the voice cannot move shown as the system's.
  const system = { id: "", label: "", system: true };
  assert.deepEqual(plain(s.devices), {
    inputs: [system, { id: "CoreAudio:mbp", label: "MacBook Pro Microphone" }, { id: "CoreAudio:usb", label: "", number: 2 }],
    outputs: [system, { id: "CoreAudio:speakers", label: "Speakers" }],
    inputId: "CoreAudio:usb", outputId: "", available: true, outputAvailable: false, busy: false });
  store.set({ facts: { audioDevices: { ...audioDevices, input: null, outputSelectable: true } } });
  s = api.snapshot();
  assert.deepEqual([s.devices.inputId, s.devices.outputId, s.devices.outputAvailable], ["", "CoreAudio:speakers", true]);
  store.set({ facts: { audioDevices: null } });
  assert.equal(api.snapshot().devices, null, "none until the voice has listed them");

  assert.equal(s.title, "mini", "the conversation the call is on, not the transcript being browsed");
  const before = Date.now();
  store.set({ call: { joined: true }, callCard: { agent: "working", youTalking: true, canSkip: false, title: "mini" } });
  s = api.snapshot();
  assert.ok(s.since >= before, "the clock starts when the call is joined");
  assert.equal(api.snapshot().since, s.since, "and keeps that start");
  assert.deepEqual([s.agent, s.youTalking], ["working", true]);

  // Relayed as the room projects it: both can speak at once.
  store.set({ callCard: { agent: "speaking", youTalking: true, canSkip: true, title: "mini" } });
  s = api.snapshot();
  assert.deepEqual([s.agent, s.youTalking, s.canSkip], ["speaking", true, true]);
  store.set({ callCard: { agent: "dancing", title: "mini" } });
  assert.equal(api.snapshot().agent, "idle", "only the states the card knows");

  store.set({ call: { joined: false } });
  assert.equal(api.snapshot().since, null);
  await flush();
  assert.ok(calls.every(([cmd]) => cmd === "bridge_state"));
});

test("the card's commands: skip, switch conversation and choose a device, each only when it can", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  const did = [];
  const store = fakeStore({ call: { joined: true }, mic: { enabled: true }, participants: [{ threadId: "a" }, { threadId: "b" }, { threadId: "c", available: false }],
    callCard: { canSkip: false } });
  win.sidevoiceUI = { store };
  win.sidevoiceActions = {
    toggleMic() {}, toggleCall: async () => {},
    skipReply: async () => did.push("skip"),
    selectParticipant: (id) => did.push("select " + id),
    setAudioDevices: async (choice) => did.push("devices " + JSON.stringify(choice)),
  };
  const api = install(win, ORIGIN);
  assert.equal(api.run({ command: "skip-reply" }), false, "nothing playing");
  store.set({ callCard: { canSkip: true } });
  assert.equal(api.run({ command: "skip-reply" }), true);
  assert.equal(api.run({ command: "select-participant", threadId: "b" }), true);
  assert.equal(api.run({ command: "select-participant", threadId: "zzz" }), false, "only a conversation the room lists");
  assert.equal(api.run({ command: "select-participant", threadId: "c" }), true, "its agent away, the call still goes to it");
  assert.equal(api.run({ command: "select-audio-device", kind: "input", id: "mbp" }), true);
  assert.equal(api.run({ command: "select-audio-device", kind: "output", id: "" }), true, "the system's default");
  assert.equal(api.run({ command: "select-audio-device", kind: "camera", id: "x" }), false);
  assert.equal(api.run({ command: "select-audio-device", kind: "input", id: 3 }), false);
  await flush();
  assert.deepEqual(did, ["skip", "select b", "select c", 'devices {"input":"mbp"}', 'devices {"output":null}']);
});

test("the card's choices go to the room one after another, busy until the last has been answered, refused or not", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const audioDevices = { inputs: [{ id: "mbp", label: "mbp" }], outputs: [], input: null, output: null, outputSelectable: false };
  const store = fakeStore({ call: { joined: true }, mic: { enabled: true }, participants: [], callCard: {}, facts: { audioDevices } });
  win.sidevoiceUI = { store };
  const answers = [];
  const asked = [];
  const plain = (value) => JSON.parse(JSON.stringify(value)); // objects made in the bridge's own realm
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {},
    setAudioDevices: (choice) => new Promise((resolve, reject) => { asked.push(choice); answers.push({ resolve, reject }); }) };
  const api = install(win, ORIGIN);
  await flush();
  const busy = () => calls.filter(([cmd]) => cmd === "bridge_state").at(-1)[1].snapshot.devices.busy;
  assert.equal(busy(), false);
  assert.equal(api.run({ command: "select-audio-device", kind: "input", id: "mbp" }), true);
  await flush();
  assert.equal(busy(), true, "told to the card at once");
  assert.equal(api.run({ command: "select-audio-device", kind: "input", id: "" }), true);
  await flush();
  assert.deepEqual(plain(asked), [{ input: "mbp" }], "the second waits for the first's answer");
  answers[0].reject(Object.assign(Error("no"), { code: "audio-device-unknown" }));
  await flush();
  assert.deepEqual(plain(asked), [{ input: "mbp" }, { input: null }], "then goes, the first refused or not");
  assert.equal(busy(), true, "until every choice is answered");
  answers[1].resolve({});
  await flush();
  assert.equal(busy(), false);
  assert.equal(api.run({ command: "select-audio-device", kind: "output", id: "x" }), true);
  await flush();
  assert.deepEqual(plain(asked.at(-1)), { output: "x" }, "a later choice goes at once");
});

test("the microphone's level reaches the app during a call only, throttled, and 0 when it stops", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  let publish = null;
  const store = fakeStore({ call: { joined: false }, mic: { enabled: true } });
  win.sidevoiceUI = { store, micLevel: { subscribe(fn) { publish = fn; return () => {}; } } };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {} };
  install(win, ORIGIN);
  publish(40);
  await flush();
  assert.equal(calls.filter(([cmd]) => cmd === "bridge_level").length, 0, "no call, no level");
  store.set({ call: { joined: true } });
  publish(40.4);
  publish(55); // within 80 ms of the last: dropped
  publish(0); // a stop always goes
  await flush();
  assert.deepEqual(calls.filter(([cmd]) => cmd === "bridge_level").map(([, args]) => args.level), [40, 0]);
});

test("installs once per page and survives a missing Tauri runtime", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  delete win.__TAURI_INTERNALS__;
  const first = install(win, ORIGIN);
  assert.equal(install(win, ORIGIN), first);
  await flush();
});

test("the engine is exactly the contract: its catalogues and keys, what runs here, what is on disk, installs and memory", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const capabilities = { runs: "native", os: "macos", arch: "aarch64", has: ["cpu", "metal", "remote"], memory_mb: 16384 };
  const answers = {
    engine_capabilities: capabilities,
    engine_installed: [{ model: "whisper-tiny", engine: "sherpa-onnx" }],
    engine_memory: { total_mb: 16384, available_mb: 9000 },
    engine_install: () => new Promise((resolve) => setTimeout(() => resolve(null), 20)), // long enough to poll
    engine_progress: { job: "?", model: "whisper-small", engine: "sherpa-onnx", done: 5, total: 10, bytes_per_s: null },
  };
  win.__TAURI_INTERNALS__.invoke = (cmd, args, options) => {
    calls.push([cmd, args, options]);
    return Promise.resolve(typeof answers[cmd] === "function" ? answers[cmd]() : answers[cmd]);
  };
  const engine = install(win, ORIGIN).host.engine;
  assert.equal(install(win, ORIGIN).host.app, "sidevoice-desktop");
  assert.deepEqual(Object.keys(engine).sort(), [
    "capabilities", "catalogs", "hasCredential", "install", "installed", "memory", "refresh", "setCredential",
    "testCredential", "uninstall",
  ]);
  assert.ok(Object.isFrozen(engine));

  assert.equal(JSON.stringify(await engine.capabilities()), JSON.stringify(capabilities));
  assert.equal(JSON.stringify(await engine.installed()), JSON.stringify([{ model: "whisper-tiny", engine: "sherpa-onnx" }]));
  assert.equal(JSON.stringify(await engine.memory()), '{"total_mb":16384,"available_mb":9000}');

  // sidevoice-web's `ModelCatalogs.install(model, {build, engine, onProgress, signal})`, as its settings call it.
  const seen = [];
  const build = "whisper-small/sherpa-onnx-int8";
  assert.equal(await engine.install("whisper-small", { build, engine: "sherpa-onnx", onProgress: (p) => seen.push(p) }), undefined);
  const installArgs = calls.find(([cmd]) => cmd === "engine_install")[1];
  assert.deepEqual(Object.keys(installArgs).sort(), ["build", "engine", "job", "model"]);
  assert.deepEqual([installArgs.model, installArgs.build, installArgs.engine], ["whisper-small", build, "sherpa-onnx"]);
  assert.match(installArgs.job, /^install-/, "the call's own job");
  assert.equal(calls.find(([cmd]) => cmd === "engine_progress")[1].job, installArgs.job, "polled by that job");
  assert.equal(JSON.stringify(seen[0]), '{"fraction":0.5,"done":5,"total":10}', "the app's bytes as the share done");
  // With nothing named the app installs the build the catalogue recommends; without onProgress nothing is polled.
  const polled = calls.filter(([cmd]) => cmd === "engine_progress").length;
  await engine.install("whisper-tiny");
  const bare = calls.filter(([cmd]) => cmd === "engine_install")[1][1];
  assert.deepEqual([bare.model, bare.build, bare.engine], ["whisper-tiny", null, null]);
  assert.notEqual(bare.job, installArgs.job, "a job per call");
  assert.equal(calls.filter(([cmd]) => cmd === "engine_progress").length, polled);
});

test("install progress: the share done of the app's bytes, null while the size is unknown", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  const reports = [
    { job: "j", model: "m", engine: "e", done: 0, total: 0, bytes_per_s: null },
    { job: "j", model: "m", engine: "e", done: 30, total: 120, bytes_per_s: null },
    { job: "j", model: "m", engine: "e", done: 130, total: 120, bytes_per_s: 7 },
  ];
  let finish;
  win.__TAURI_INTERNALS__.invoke = (cmd) => {
    if (cmd === "engine_install") return new Promise((resolve) => { finish = resolve; });
    if (cmd === "engine_progress") return Promise.resolve(reports.length > 1 ? reports.shift() : reports[0]);
    return Promise.resolve(null);
  };
  const seen = [];
  const installing = install(win, ORIGIN).host.engine.install("m", { onProgress: (p) => seen.push(JSON.stringify(p)) });
  while (seen.length < 3) await new Promise((resolve) => setTimeout(resolve, 5));
  finish(null);
  await installing;
  assert.deepEqual(seen.slice(0, 3), [
    '{"fraction":null,"done":0,"total":0}',
    '{"fraction":0.25,"done":30,"total":120}',
    '{"fraction":1,"done":130,"total":120}',
  ]);
});

test("host.engine lists the catalogues and keeps provider keys, refusing with a code", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const catalogs = [
    { id: "local", name: null, status: { stale: false }, models: [{
      id: "whisper-tiny", capabilities: ["stt"], builds: [
        { id: "whisper-tiny/whisper-cpp", accelerator: "metal", acceleratorClass: "gpu" },
        { id: "whisper-tiny/sherpa-onnx", accelerator: "cpu", acceleratorClass: "cpu" },
        { id: "whisper-tiny/unavailable" },
      ],
    }] },
    { id: "openai", name: "OpenAI", status: { stale: false, reason: { code: "credential-missing", params: {} } }, models: [] },
  ];
  const answers = { engine_catalogs: catalogs, engine_set_credential: null, engine_has_credential: true };
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    if (cmd === "engine_set_credential" && args.provider === "Nope") {
      return Promise.reject({ key: "provider_invalid", provider: "Nope", message: "Not a provider id." });
    }
    return Promise.resolve(answers[cmd]);
  };
  const engine = install(win, ORIGIN).host.engine;
  const listed = await engine.catalogs();
  assert.equal(JSON.stringify(listed), JSON.stringify(catalogs));
  assert.deepEqual(listed[0].models[0].builds.map((build) => build.acceleratorClass), ["gpu", "cpu", undefined]);
  assert.equal(await engine.setCredential("openai", "sk-test"), undefined);
  await engine.setCredential("openai", null);
  assert.equal(await engine.hasCredential("openai"), true);
  const refused = await engine.setCredential("Nope", "x").catch((error) => error);
  assert.deepEqual([refused.key, refused.code], ["provider_invalid", "provider-invalid"]);
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("engine_")), [
    ["engine_catalogs", undefined],
    ["engine_set_credential", { provider: "openai", key: "sk-test" }],
    ["engine_set_credential", { provider: "openai", key: null }],
    ["engine_has_credential", { provider: "openai" }],
    ["engine_set_credential", { provider: "Nope", key: "x" }],
  ]);
});

test("host.engine.uninstall takes a model off the computer, refusing with the engine's code", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const refusals = {
    "kokoro-82m-v1.0": { key: "model_in_use", model: "kokoro-82m-v1.0", code: "model-in-use", message: "kokoro-82m-v1.0 is in use by the voice call." },
    "whisper-small": { key: "model_installing", model: "whisper-small", code: "model-installing", message: "whisper-small is being downloaded; cancel the download to remove it." },
    nope: { key: "model_unknown", model: "nope", code: "model-not-found", message: "Unknown model nope." },
  };
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    return refusals[args?.model] ? Promise.reject(refusals[args.model]) : Promise.resolve(null);
  };
  const engine = install(win, ORIGIN).host.engine;
  assert.equal(await engine.uninstall("whisper-tiny"), undefined);
  for (const [model, refusal] of Object.entries(refusals)) {
    const error = await engine.uninstall(model).catch((failure) => failure);
    assert.deepEqual([error.key, error.code, error.model, error.message], [refusal.key, refusal.code, model, refusal.message]);
  }
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("engine_")), [
    ["engine_uninstall", { model: "whisper-tiny" }],
    ["engine_uninstall", { model: "kokoro-82m-v1.0" }],
    ["engine_uninstall", { model: "whisper-small" }],
    ["engine_uninstall", { model: "nope" }],
  ]);
});

test("host.engine tries a key without keeping it, answering the catalogue's reason or null, and asks a provider again", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const rejected = { code: "credential-rejected", params: {}, detail: "Incorrect API key provided: [key]." };
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    if (cmd === "engine_test_credential") {
      if (args.provider === "linux") return Promise.reject({ key: "credentials_unavailable", message: "No keys here." });
      return Promise.resolve(args.key === "sk-good" ? null : rejected);
    }
    if (cmd === "engine_refresh" && args.provider === "nope") {
      return Promise.reject({ key: "catalog_not_found", catalog: "nope", message: "There is no catalogue nope." });
    }
    return Promise.resolve(null);
  };
  const engine = install(win, ORIGIN).host.engine;
  assert.deepEqual(await engine.testCredential("openai", "sk-bad"), rejected);
  assert.equal(await engine.testCredential("openai", "sk-good"), null);
  const unavailable = await engine.testCredential("linux", "k").catch((error) => error);
  assert.equal(unavailable.code, "credentials-unavailable");
  assert.equal(await engine.refresh("openai"), undefined);
  const missing = await engine.refresh("nope").catch((error) => error);
  assert.deepEqual([missing.code, missing.catalog], ["catalog-not-found", "nope"]);
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("engine_")), [
    ["engine_test_credential", { provider: "openai", key: "sk-bad" }],
    ["engine_test_credential", { provider: "openai", key: "sk-good" }],
    ["engine_test_credential", { provider: "linux", key: "k" }],
    ["engine_refresh", { provider: "openai" }],
    ["engine_refresh", { provider: "nope" }],
  ]);
  assert.ok(!calls.some(([cmd]) => cmd === "engine_set_credential"), "a key tried is never kept");
});

// ---- host.voice: the voice call the app runs (docs/BRIDGE.md → "The voice call") ----

function voiceWindow(answers = {}) {
  const { win, calls } = fakeWindow(ORIGIN);
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    const answer = answers[cmd];
    if (answer instanceof Error) return Promise.reject(answer.refusal);
    return Promise.resolve(answer);
  };
  return { win, calls };
}

// A call works once the app says so and its first state that is not idle has reached the page.
async function works(api, ...calls) {
  await flush();
  for (const call of calls) api.voiceEvent({ key: call.id, type: "state", data: { listening: "listening" } });
  await Promise.all(calls.map((call) => call.ready));
}

const LISTEN = { input: { stt: { catalog: "local", model: "whisper" }, endOfTurn: { kind: "silence", ms: 1500 } } };
const SPEAK = { output: { tts: { catalog: "local", model: "kokoro" } } };
const BOTH = { ...LISTEN, ...SPEAK };

test("native voice offers the shared host and independent calls", async () => {
  const install = loadFactory();
  for (const offered of [undefined, false, "__SIDEVOICE_VOICE__", "true"]) {
    assert.equal("voice" in install(fakeWindow(ORIGIN).win, ORIGIN, null, true, offered).host, false);
  }
  const { win, calls } = voiceWindow();
  const api = install(win, ORIGIN, "native", true, true);
  const host = api.host.voice;
  assert.deepEqual(Object.keys(host).sort(), ["audioDevices", "onAudioDevices", "preload", "routeInput", "setAudioDevices", "start"]);
  const a = host.start(BOTH);
  const b = host.start(LISTEN);
  assert.notEqual(a.id, b.id);
  assert.deepEqual(Object.keys(a).sort(), ["cancelInput", "endInput", "id", "mute", "onError", "onLevel",
    "onPlaybackLevel", "onState", "onTake", "onTurn", "ready", "say", "stop", "update"]);
  assert.ok(Object.isFrozen(host) && Object.isFrozen(a));
  assert.ok(a.ready instanceof Promise);
  await host.routeInput(a);
  await host.routeInput(b);
  await host.routeInput(null);
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_route_input"), [
    ["voice_route_input", { key: a.id }], ["voice_route_input", { key: b.id }], ["voice_route_input", { key: null }],
  ]);
  await assert.rejects(host.routeInput({ id: a.id }), { name: "TypeError" });
  await a.stop();
  await b.stop();
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_dispose_call").map(([, args]) => args.key), [a.id, b.id]);
  await assert.rejects(host.routeInput(a), { name: "TypeError" }, "a stopped call takes no microphone");
});

test("start makes the lane, the call with its halves and its take, then asks when it works; settings with no half throw", async () => {
  const { win, calls } = voiceWindow();
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const host = api.host.voice;
  for (const bad of [undefined, null, "words", {}, { input: null, output: null }, { idle_unload_minutes: 5 }]) {
    assert.throws(() => host.start(bad), { name: "TypeError" });
  }
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("voice_")), [], "nothing was asked of the app");
  const listening = host.start(LISTEN, { take: true });
  const speaking = host.start(SPEAK);
  await works(api, listening, speaking);
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("voice_")).map(([cmd, args]) => [cmd, args.key, args.take, args.settings]), [
    ["voice_create_call", listening.id, undefined, undefined],
    ["voice_create_call", speaking.id, undefined, undefined],
    ["voice_start", listening.id, true, plain(LISTEN)],
    ["voice_start", speaking.id, false, plain(SPEAK)],
    ["voice_ready", listening.id, undefined, undefined],
    ["voice_ready", speaking.id, undefined, undefined],
  ]);
});

test("a call the app refuses rejects ready with the refusal, says nothing was played, and stop releases its lane", async () => {
  const refusal = { key: "voice_model_unknown", code: "model-unknown", model: "whisper", message: "no such model" };
  const { win, calls } = voiceWindow({ voice_start: Object.assign(new Error(), { refusal }) });
  const unhandled = [];
  const onUnhandled = (reason) => unhandled.push(reason);
  process.on("unhandledRejection", onUnhandled);
  const call = loadFactory()(win, ORIGIN, "native", true, true).host.voice.start(BOTH);
  await assert.rejects(call.ready, (error) => error === refusal);
  const said = call.say("Hello");
  assert.deepEqual(plain(await said.outcome), { status: "not-played", reason: "failed", code: "model-unknown" });
  await assert.rejects(call.update({ output: null }), (error) => error === refusal);
  await call.stop();
  await new Promise((resolve) => setTimeout(resolve, 20));
  process.off("unhandledRejection", onUnhandled);
  assert.deepEqual(unhandled, [], "a ready nobody reads is not an unhandled rejection");
  assert.ok(!calls.some(([cmd]) => cmd === "voice_say" || cmd === "voice_update"), "nothing is asked of a call that was not made");
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_dispose_call").map(([, args]) => args.key), [call.id]);
});

test("a call that fails before it works rejects ready with the native code", async () => {
  const refusal = { key: "voice_failed", code: "microphone-denied", message: "The call could not start." };
  const { win } = voiceWindow({ voice_ready: Object.assign(new Error(), { refusal }) });
  const call = loadFactory()(win, ORIGIN, "native", true, true).host.voice.start(BOTH);
  await assert.rejects(call.ready, (error) => error === refusal);
  await call.stop();
});

test("ready resolves once the app says the call works and the page has its first state that is not idle", async () => {
  const { win } = voiceWindow({ voice_stop: true });
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const call = api.host.voice.start(BOTH);
  let works = false;
  call.ready.then(() => { works = true; });
  await flush();
  await flush();
  assert.equal(works, false, "the app answered, the state has not reached the page");
  api.voiceEvent({ key: call.id, type: "state", data: { listening: "idle" } });
  await flush();
  assert.equal(works, false, "idle is not working");
  api.voiceEvent({ key: call.id, type: "state", data: { listening: "listening" } });
  await call.ready;
  // One that is stopped before its state came never works.
  const stopped = api.host.voice.start(SPEAK);
  await flush();
  const rejected = assert.rejects(stopped.ready, { code: "stopped" });
  const stopping = stopped.stop();
  await flush();
  await flush();
  api.voiceEvent({ key: stopped.id, type: "state", data: { listening: "idle" } });
  await stopping;
  await rejected;
});

test("per-call commands carry the call key, wait for the call, and say handles remain separate", async () => {
  const { win, calls } = voiceWindow();
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const a = api.host.voice.start(BOTH);
  const b = api.host.voice.start(BOTH);
  const saidA = a.say("A");
  const saidB = b.say("B");
  assert.equal(saidA.id, saidB.id, "say IDs may repeat on different calls");
  await works(api, a, b);
  await flush();
  const order = (key) => calls.filter(([, args]) => args && args.key === key).map(([cmd]) => cmd);
  assert.deepEqual(order(a.id).slice(0, 3), ["voice_create_call", "voice_start", "voice_ready"]);
  assert.ok(order(a.id).indexOf("voice_say") > order(a.id).indexOf("voice_start"), "said once the call was made");
  api.voiceEvent({ key: b.id, type: "say", data: { key: saidB.id, event: { type: "done", outcome: {
    status: "heard-up-to", heard_chars: 1, reason: { reason: "cancelled" },
  } } } });
  api.voiceEvent({ key: a.id, type: "say", data: { key: saidA.id, event: { type: "done", outcome: {
    status: "not-played", reason: { reason: "failed", code: "speaker-unavailable" },
  } } } });
  assert.deepEqual(plain(await saidB.outcome), { status: "heard-up-to", heard_chars: 1, reason: "cancelled" });
  assert.deepEqual(plain(await saidA.outcome), { status: "not-played", reason: "failed", code: "speaker-unavailable" });
  assert.equal(a.mute(true), undefined);
  assert.equal(b.cancelInput(), undefined);
  await flush();
  await a.stop();
  const commands = calls.filter(([cmd]) => cmd.startsWith("voice_") && !["voice_create_call"].includes(cmd));
  assert.ok(commands.every(([, args]) => args && (args.key === a.id || args.key === b.id)));
  assert.deepEqual(commands.filter(([cmd]) => cmd === "voice_say").map(([, args]) => [args.key, args.sayKey]),
    [[a.id, saidA.id], [b.id, saidB.id]]);
  assert.ok(commands.some(([cmd, args]) => cmd === "voice_mute" && args.key === a.id && args.muted));
  assert.ok(commands.some(([cmd, args]) => cmd === "voice_cancel_input" && args.key === b.id));
});

test("update sends each change to the call, one after the other, and refuses a malformed one", async () => {
  const { win, calls } = voiceWindow();
  const call = loadFactory()(win, ORIGIN, "native", true, true).host.voice.start(SPEAK);
  for (const bad of [undefined, null, "words", 7]) {
    await assert.rejects(call.update(bad), { name: "TypeError" });
  }
  // A microphone appears: the input half is added; then it is lost again: removed.
  const first = call.update({ input: LISTEN.input });
  const second = call.update({ input: null, output: { tts: { catalog: "local", model: "other" }, voice: "af_heart" } });
  assert.equal(await first, undefined);
  await second;
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_update").map(([, args]) => [args.key, args.change]), [
    [call.id, plain({ input: LISTEN.input })],
    [call.id, plain({ input: null, output: { tts: { catalog: "local", model: "other" }, voice: "af_heart" } })],
  ]);
  assert.ok(calls.findIndex(([cmd]) => cmd === "voice_update") > calls.findIndex(([cmd]) => cmd === "voice_start"));
  // The app refuses one that leaves nothing, or a model it cannot fill: the call rejects with it, and the next goes on.
  const refusal = { key: "voice_halves_missing", code: "halves-missing", message: "A call needs a half." };
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, plain(args)]);
    return cmd === "voice_update" && args.change.output === null ? Promise.reject(refusal) : Promise.resolve();
  };
  await assert.rejects(call.update({ output: null }), (error) => error === refusal);
  await call.update({ input: LISTEN.input });
  await call.stop();
  await assert.rejects(call.update({ output: SPEAK.output }), { code: "stopped" });
});

test("preload asks the app to load the models of the settings and rejects what it refuses", async () => {
  const refusal = { key: "voice_failed", code: "model-load-failed", message: "The models could not be loaded." };
  const { win, calls } = voiceWindow();
  const host = loadFactory()(win, ORIGIN, "native", true, true).host.voice;
  for (const bad of [undefined, null, {}, { input: null }]) {
    await assert.rejects(host.preload(bad), { name: "TypeError" });
  }
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("voice_")), []);
  assert.equal(await host.preload({ ...BOTH, idle_unload_minutes: 3 }), undefined);
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("voice_")),
    [["voice_preload", { settings: plain({ ...BOTH, idle_unload_minutes: 3 }) }]]);
  win.__TAURI_INTERNALS__.invoke = () => Promise.reject(refusal);
  await assert.rejects(host.preload(SPEAK), (error) => error === refusal);
});

test("A to B routing keeps delayed per-call turns on A despite a later switch", async () => {
  const { win, calls } = voiceWindow();
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const host = api.host.voice;
  const a = host.start(BOTH);
  const b = host.start(BOTH);
  const seen = [];
  a.onTurn((turn) => seen.push(["A", turn.phase, turn.text]));
  b.onTurn((turn) => seen.push(["B", turn.phase, turn.text]));
  await host.routeInput(a);
  // Native accepted A's speech, but webview.eval delivers its start after the route to B.
  const delayedA = { key: a.id, type: "turn", data: { phase: "started", turn_id: "a-1", started_at: 1 } };
  await host.routeInput(b);
  api.voiceEvent(delayedA);
  api.voiceEvent({ key: b.id, type: "turn", data: { phase: "started", turn_id: "b-1", started_at: 2 } });
  api.voiceEvent({ key: b.id, type: "turn", data: { phase: "finished", turn_id: "b-1", text: "B words" } });
  api.voiceEvent({ key: a.id, type: "turn", data: { phase: "finished", turn_id: "a-1", text: "A words" } });
  assert.deepEqual(seen, [["A", "started", undefined], ["B", "started", undefined],
    ["B", "finished", "B words"], ["A", "finished", "A words"]]);
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_route_input").map(([, args]) => args.key), [a.id, b.id]);
});

test("per-call state, levels, takes and saying outcomes are isolated after stop", async () => {
  const { win } = voiceWindow({ voice_stop: true });
  win.atob = atob;
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const a = api.host.voice.start(BOTH, { take: true });
  const b = api.host.voice.start(BOTH);
  const heard = [];
  a.onLevel((level) => heard.push(["A", "mic", level]));
  b.onPlaybackLevel((level) => heard.push(["B", "speaker", level]));
  api.voiceEvent({ key: a.id, type: "level", data: 0.7 });
  api.voiceEvent({ key: b.id, type: "playback-level", data: 0.3 });
  api.voiceEvent({ key: b.id, type: "playback-level", data: 0 });
  assert.deepEqual(heard, [["A", "mic", 0.7], ["B", "speaker", 0.3], ["B", "speaker", 0]]);
  const takes = [];
  a.onTake((take) => takes.push(take));
  await works(api, a);
  let stopped = false;
  const stopping = a.stop().then(() => { stopped = true; });
  await flush();
  assert.equal(stopped, false, "no idle has answered the stop yet");
  const samples = new Float32Array([0, 0.5]);
  api.voiceEvent({ key: a.id, type: "take", data: { sampleRate: 16000,
    samples: Buffer.from(samples.buffer).toString("base64"), started_at: 1, ended_at: 2, turns: [] } });
  assert.equal(stopped, false);
  api.voiceEvent({ key: a.id, type: "state", data: { listening: "idle" } });
  await stopping;
  assert.deepEqual([...takes[0].samples], [...samples], "the take came before the stop resolved");
  api.voiceEvent({ key: a.id, type: "level", data: 0.9 });
  api.voiceEvent({ key: b.id, type: "playback-level", data: 0.5 });
  assert.deepEqual(heard.at(-1), ["B", "speaker", 0.5]);
  assert.equal(a.stop(), a.stop(), "a second stop answers the first's promise");
});

test("mute and cancelInput never reject, and mutes keep their order", async () => {
  const { win, calls } = fakeWindow(ORIGIN);
  const answered = [];
  const unhandled = [];
  const onUnhandled = (reason) => unhandled.push(reason);
  process.on("unhandledRejection", onUnhandled);
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    if (cmd === "voice_mute") {
      // The first mute is slow and refused; a later one must not overtake it.
      const slow = calls.filter(([name]) => name === "voice_mute").length === 1;
      return new Promise((resolve, reject) => setTimeout(() => {
        answered.push(args.muted);
        if (slow) reject({ key: "bad_request", message: "Unknown voice call." });
        else resolve(null);
      }, slow ? 20 : 0));
    }
    if (cmd === "voice_cancel_input") return Promise.reject({ key: "bad_request", message: "Unknown voice call." });
    return Promise.resolve(null);
  };
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const a = api.host.voice.start(BOTH);
  assert.equal(a.mute(true), undefined);
  assert.equal(a.mute(false), undefined);
  assert.equal(a.mute(true), undefined);
  assert.equal(a.cancelInput(), undefined);
  await works(api, a);
  await new Promise((resolve) => setTimeout(resolve, 60));
  process.off("unhandledRejection", onUnhandled);
  assert.deepEqual(unhandled, []);
  assert.deepEqual(answered, [true, false, true]);
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_mute").map(([, args]) => [args.key, args.muted]),
    [[a.id, true], [a.id, false], [a.id, true]]);
});

test("endInput is asked in order with mutes, resolves once native closed the turn, and never rejects", async () => {
  const { win, calls } = fakeWindow(ORIGIN);
  const closing = [];
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    if (cmd === "voice_mute") return new Promise((resolve) => setTimeout(() => resolve(null), 20));
    if (cmd === "voice_end_input") return new Promise((resolve) => closing.push(resolve));
    if (cmd === "voice_stop") return Promise.resolve(false);
    return Promise.resolve(null);
  };
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const a = api.host.voice.start(BOTH);
  a.mute(true);
  let ended = false;
  const ending = a.endInput().then((value) => { ended = true; return value; });
  await flush();
  assert.ok(!calls.some(([cmd]) => cmd === "voice_end_input"), "asked after the slow mute before it");
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_mute" || cmd === "voice_end_input").map(([cmd, args]) => [cmd, args.key]),
    [["voice_mute", a.id], ["voice_end_input", a.id]]);
  assert.equal(ended, false, "it waits for native to close the turn");
  closing.shift()(null);
  assert.equal(await ending, undefined);
  // A native refusal never rejects, and a stop does not wait for it.
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    if (cmd === "voice_end_input") return Promise.reject({ key: "bad_request", message: "Unknown voice call." });
    return Promise.resolve(cmd === "voice_stop" ? false : null);
  };
  assert.equal(await a.endInput(), undefined);
  await a.stop();
  assert.equal(await a.endInput(), undefined, "after stop, nothing to end");
});

test("stop delivers the call's own ends before it resolves: the partial saying, the open turn and the take", async () => {
  const { win, calls } = voiceWindow({ voice_stop: true });
  win.atob = atob;
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const host = api.host.voice;
  const a = host.start(BOTH, { take: true });
  const turns = [];
  const takes = [];
  a.onTurn((turn) => turns.push(turn.phase));
  a.onTake((take) => takes.push(take));
  await works(api, a);
  const said = a.say("Hello there, again.");
  const steps = [];
  said.onEvent((event) => steps.push(event.type));
  await flush();
  api.voiceEvent({ key: a.id, type: "say", data: { key: said.id, event: { type: "playing" } } });
  api.voiceEvent({ key: a.id, type: "say", data: { key: said.id, event: { type: "progress", heard_chars: 8 } } });
  api.voiceEvent({ key: a.id, type: "turn", data: { phase: "started", turn_id: "a-1", started_at: 1 } });
  let ended = false;
  const stopping = a.stop().then(() => { ended = true; });
  await flush();
  assert.ok(calls.some(([cmd, args]) => cmd === "voice_stop" && args.key === a.id), "the call is stopped first");
  assert.ok(!calls.some(([cmd]) => cmd === "voice_dispose_call"), "the lane is kept until the call's ends came");
  assert.equal(ended, false);
  // What native stop answers, in its order: the saying's end, the open turn's, the take, then the idle state.
  api.voiceEvent({ key: a.id, type: "say", data: { key: said.id, event: { type: "done", outcome: {
    status: "heard-up-to", heard_chars: 8, reason: { reason: "stopped" },
  } } } });
  api.voiceEvent({ key: a.id, type: "turn", data: { phase: "cancelled", turn_id: "a-1", merged: false } });
  api.voiceEvent({ key: a.id, type: "take", data: { sampleRate: 16000,
    samples: Buffer.from(new Float32Array([0.5]).buffer).toString("base64"), started_at: 1, ended_at: 2, turns: [] } });
  api.voiceEvent({ key: a.id, type: "state", data: { listening: "idle" } });
  await stopping;
  assert.deepEqual(plain(await said.outcome), { status: "heard-up-to", heard_chars: 8, reason: "stopped" });
  assert.deepEqual(steps, ["playing", "progress", "done"], "one terminal, the call's own");
  assert.deepEqual(turns, ["started", "cancelled"]);
  assert.equal(takes.length, 1);
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_dispose_call").map(([, args]) => args.key), [a.id]);
  // Nothing reaches the stopped call, nor a call made after it.
  const b = host.start(BOTH);
  const late = [];
  b.onTurn((turn) => late.push(turn));
  api.voiceEvent({ key: a.id, type: "turn", data: { phase: "started", turn_id: "a-2", started_at: 3 } });
  api.voiceEvent({ key: a.id, type: "say", data: { key: said.id, event: { type: "done", outcome: { status: "heard" } } } });
  assert.deepEqual(turns, ["started", "cancelled"]);
  assert.deepEqual(late, []);
  assert.deepEqual(steps, ["playing", "progress", "done"]);
  assert.deepEqual(plain(await a.say("late").outcome), { status: "not-played", reason: "stopped" });
});

test("shared audio devices stay at host scope", async () => {
  const devices = { inputs: [], outputs: [], input: null, output: null, outputSelectable: true };
  const { win, calls } = voiceWindow({ voice_audio_devices: devices, voice_set_audio_devices: devices });
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const seen = [];
  const stop = api.host.voice.onAudioDevices((value) => seen.push(value));
  assert.deepEqual(plain(await api.host.voice.audioDevices()), devices);
  await api.host.voice.setAudioDevices({ input: null });
  api.voiceEvent({ type: "devices", data: devices });
  stop();
  api.voiceEvent({ type: "devices", data: devices });
  assert.equal(seen.length, 1);
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_set_audio_devices"),
    [["voice_set_audio_devices", { choice: { input: null } }]]);
});

test("cancel: the signal's abort cancels the install's own job, which rejects install-cancelled", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const pending = new Map();
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, structuredClone(args)]);
    if (cmd === "engine_install") return new Promise((resolve, reject) => pending.set(args.job, { resolve, reject }));
    if (cmd === "engine_cancel") {
      const job = pending.get(args.job);
      if (!job) return Promise.resolve(false);
      pending.delete(args.job);
      job.reject({ key: "install_cancelled", message: "The download was cancelled." });
      return Promise.resolve(true);
    }
    return Promise.resolve(null);
  };
  const engine = install(win, ORIGIN).host.engine;
  const seen = [];
  const abort = new AbortController();
  const installing = engine.install("whisper-small", { engine: "sherpa-onnx", onProgress: (p) => seen.push(p), signal: abort.signal });
  await flush();
  abort.abort();
  await assert.rejects(installing, (error) =>
    error.code === "install-cancelled" && error.key === "install_cancelled" && error.message === "The download was cancelled.");
  const job = calls.find(([cmd]) => cmd === "engine_install")[1].job;
  assert.deepEqual(calls.filter(([cmd]) => cmd === "engine_cancel").map(([, args]) => args), [{ job }]);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(seen, [], "no progress for it once it ended");

  // Aborted in the same tick as the call: the cancel goes behind the install, never ahead of it.
  const now = new AbortController();
  const immediate = engine.install("kokoro-82m-v1.0", { signal: now.signal });
  now.abort();
  await assert.rejects(immediate, (error) => error.code === "install-cancelled");
  const order = calls.filter(([cmd]) => cmd === "engine_install" || cmd === "engine_cancel").slice(-2);
  assert.deepEqual(order.map(([cmd]) => cmd), ["engine_install", "engine_cancel"]);
  assert.equal(order[0][1].job, order[1][1].job);

  // An abort after the install ended asks the app nothing.
  const late = new AbortController();
  const ended = engine.install("whisper-tiny", { signal: late.signal });
  await flush();
  [...pending.values()][0].resolve(null);
  await ended;
  late.abort();
  await flush();
  assert.equal(calls.filter(([cmd]) => cmd === "engine_cancel").length, 2);
});

test("an install whose signal is already aborted rejects install-cancelled at once, asking the app nothing", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const abort = new AbortController();
  abort.abort();
  const progress = [];
  await assert.rejects(
    install(win, ORIGIN).host.engine.install("whisper-small", { build: "b", onProgress: (p) => progress.push(p), signal: abort.signal }),
    (error) => error.code === "install-cancelled" && error.key === "install_cancelled",
  );
  await flush();
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("engine_")), []);
  assert.deepEqual(progress, []);
});

test("concurrent installs: each callback gets its own job's bytes, and none while its job waits", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  // The app's side, as engine_install / engine_progress keep it: a job has progress only once it has started.
  const progress = new Map();
  const finish = new Map();
  const jobs = [];
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    if (cmd === "engine_install") {
      jobs.push(args.job);
      return new Promise((resolve) => finish.set(args.job, () => { progress.delete(args.job); resolve(null); }));
    }
    if (cmd === "engine_progress") {
      const bytes = progress.get(args.job);
      return Promise.resolve(bytes ? { job: args.job, model: "?", engine: "sherpa-onnx", done: bytes[0], total: bytes[1], bytes_per_s: 1000 } : null);
    }
    return Promise.resolve(null);
  };
  const engine = install(win, ORIGIN).host.engine;
  const whisper = [];
  const kokoro = [];
  const a = engine.install("whisper-small", { engine: "sherpa-onnx", onProgress: (p) => whisper.push([p.done, p.total]) });
  const b = engine.install("kokoro-82m-v1.0", { engine: "sherpa-onnx", onProgress: (p) => kokoro.push([p.done, p.total]) });
  await flush(); // the bridge reaches the app on a later tick
  try {
    assert.equal(jobs.length, 2);
    assert.notEqual(jobs[0], jobs[1], "one job per call");
    progress.set(jobs[0], [75, 100]); // A downloads; B waits behind it
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.ok(whisper.some(([d]) => d === 75), "A sees its bytes");
    assert.deepEqual(kokoro, [], "B, still waiting, sees nothing — not A's bytes");
    finish.get(jobs[0])();
    await a;
    progress.set(jobs[1], [10, 132]);
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.ok(kokoro.every(([, t]) => t === 132) && kokoro.length > 0, "B sees only its own");
    assert.ok(whisper.every(([, t]) => t === 100));
    finish.get(jobs[1])();
    await b;
  } finally {
    for (const done of finish.values()) done(); // never leave a poller running
  }
});

test("a refusal reaches the page as the app sent it: a key, its parameters and an English message", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  const refusal = { key: "model_needs_memory", model: "whisper-large-v3", needed_mb: 4000, memory_mb: 2000,
    message: "whisper-large-v3 needs 4000 MB of memory; this machine has 2000 MB." };
  win.__TAURI_INTERNALS__.invoke = () => Promise.reject(refusal);
  const engine = install(win, ORIGIN).host.engine;
  await assert.rejects(engine.install("whisper-large-v3", { engine: "sherpa-onnx" }), (error) => {
    assert.equal(error.key, "model_needs_memory");
    assert.equal(error.code, "model-needs-memory", "the code the page reads");
    assert.equal(error.needed_mb, 4000);
    assert.equal(error.model, "whisper-large-v3");
    assert.equal(error.message, refusal.message);
    return true;
  });
});

test("works on the app's own pages, whose origin may be opaque", () => {
  const install = loadFactory();
  const { win } = fakeWindow("tauri://localhost");
  assert.ok(install(win, "tauri://localhost"));
  const other = fakeWindow("tauri://localhost");
  assert.equal(install(other.win, ORIGIN), null);
  assert.equal(win.__SIDEVOICE_TARGET__, null, "the app never tells the interface where to look first");
  assert.equal("__SIDEVOICE_TARGET__" in other.win, false, "nor anything to another origin");
});

test("the placeholders the app replaces are present exactly once", () => {
  assert.equal(source.split("__SIDEVOICE_ROOM_ORIGIN__").length, 2);
  assert.equal(source.split("__SIDEVOICE_MEDIA_KEYS__").length, 2);
  assert.equal(source.split("__SIDEVOICE_LOCAL_HOST__").length, 2);
  assert.equal(source.split("__SIDEVOICE_VOICE__").length, 2);
});

test("says who answers the media keys", () => {
  const install = loadFactory();
  assert.equal(install(fakeWindow(ORIGIN).win, ORIGIN, "native").host.mediaKeys, "native");
  assert.equal(install(fakeWindow(ORIGIN).win, ORIGIN, "__SIDEVOICE_MEDIA_KEYS__").host.mediaKeys, null);
});

// ---- host.localHost: this computer's own core (docs/LOCAL_HOST.md) ----

function localHostWindow(answers = {}) {
  const { win, calls, tick } = fakeWindow(ORIGIN);
  const cleared = [];
  win.clearInterval = (id) => cleared.push(id);
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
    const answer = answers[cmd];
    if (answer instanceof Error) return Promise.reject(answer.refusal);
    return Promise.resolve(typeof answer === "function" ? answer(args) : answer);
  };
  return { win, calls, tick, cleared };
}

test("host carries the bridge version; localHost only where the app offers it", () => {
  const install = loadFactory();
  for (const offered of [undefined, false, "__SIDEVOICE_LOCAL_HOST__", "true"]) {
    const { win } = fakeWindow(ORIGIN);
    const api = install(win, ORIGIN, null, offered);
    assert.equal(api.host.version, 2);
    assert.equal(api.host.app, "sidevoice-desktop");
    assert.ok(api.host.engine, "the rest of the host is untouched");
    assert.equal("localHost" in api.host, false, String(offered));
  }
  const { win } = fakeWindow(ORIGIN);
  const api = install(win, ORIGIN, "native", true);
  assert.deepEqual(Object.keys(api.host.localHost).sort(), [
    "cancel", "install", "pairing", "pairingCode", "reconnect", "restart", "revealLog", "serviceInstall",
    "serviceUninstall", "setStartAtLogin", "start", "state", "stop", "subscribe",
  ]);
  assert.ok(Object.isFrozen(api.host.localHost));
});

test("localHost calls go to their native commands and resolve what the app answers", async () => {
  const install = loadFactory();
  const pairing = { fp: "fp", public_key: "k", device_id: "d", token: "secret", urls: ["http://127.0.0.1:5"], host: "Mac", local: true };
  const { win, calls } = localHostWindow({
    local_host_state: { state: "running", service: "launchd", calls: 0 },
    local_host_pairing: pairing,
    local_host_action: (args) => ({ state: args.action === "stop" ? "stopped-by-person" : "running" }),
    local_host_pairing_code: { code: "SV1", expires_in: 600, reach: "direct" },
  });
  const local = install(win, ORIGIN, null, true).host.localHost;
  assert.deepEqual(await local.state(), { state: "running", service: "launchd", calls: 0 });
  assert.deepEqual(await local.pairing(), pairing);
  assert.deepEqual(await local.stop(), { state: "stopped-by-person" });
  for (const name of ["start", "restart", "serviceInstall", "serviceUninstall", "reconnect", "revealLog"]) await local[name]();
  await local.setStartAtLogin(true);
  await local.setStartAtLogin(false);
  assert.deepEqual(await local.pairingCode(), { code: "SV1", expires_in: 600, reach: "direct" });
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("local_host")), [
    ["local_host_state", undefined],
    ["local_host_pairing", undefined],
    ["local_host_action", { action: "stop" }],
    ["local_host_action", { action: "start" }],
    ["local_host_action", { action: "restart" }],
    ["local_host_action", { action: "service-install" }],
    ["local_host_action", { action: "service-uninstall" }],
    ["local_host_action", { action: "reconnect" }],
    ["local_host_action", { action: "reveal-log" }],
    ["local_host_action", { action: "start-at-login-on" }],
    ["local_host_action", { action: "start-at-login-off" }],
    ["local_host_pairing_code", undefined],
  ]);
});

test("a refused action rejects with the app's {key, message}", async () => {
  const install = loadFactory();
  const refused = Object.assign(new Error("refused"), { refusal: { key: "service.not-loaded", message: "m" } });
  const { win } = localHostWindow({ local_host_action: refused });
  await assert.rejects(install(win, ORIGIN, null, true).host.localHost.start(), { key: "service.not-loaded", message: "m" });
});

test("setStartAtLogin takes a boolean only, and a connector without it rejects with its key", async () => {
  const install = loadFactory();
  const unsupported = Object.assign(new Error("refused"), {
    refusal: { key: "service.login-unsupported", message: "m" },
  });
  const { win, calls } = localHostWindow({ local_host_action: unsupported });
  const local = install(win, ORIGIN, null, true).host.localHost;
  for (const bad of [undefined, null, 1, "true", {}]) {
    await assert.rejects(local.setStartAtLogin(bad), { key: "bad_request" }, String(bad));
  }
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("local_host")), [], "nothing runs on a bad argument");
  await assert.rejects(local.setStartAtLogin(false), { key: "service.login-unsupported", message: "m" });
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("local_host")), [
    ["local_host_action", { action: "start-at-login-off" }],
  ]);
});

test("subscribe: the state now, then only changes; polling stops with the last listener", async () => {
  const install = loadFactory();
  let state = { state: "starting" };
  const { win, calls, tick, cleared } = localHostWindow({ local_host_state: () => state });
  const local = install(win, ORIGIN, null, true).host.localHost;
  const seen = [];
  const stop = local.subscribe((s) => seen.push(s.state));
  await flush();
  assert.deepEqual(seen, ["starting"]);
  tick();
  await flush();
  assert.deepEqual(seen, ["starting"], "unchanged: not delivered again");
  state = { state: "running", calls: 0 };
  tick();
  await flush();
  assert.deepEqual(seen, ["starting", "running"]);
  const late = [];
  const stopLate = local.subscribe((s) => late.push(s.state));
  assert.deepEqual(late, ["running"], "a later listener gets the state at once");
  stop();
  assert.deepEqual(cleared, [], "still one listener");
  stopLate();
  assert.equal(cleared.length, 1, "no listener, no polling");
  const polls = calls.filter(([cmd]) => cmd === "local_host_state").length;
  tick();
  await flush();
  assert.throws(() => local.subscribe("nope"), { name: "TypeError" });
  assert.ok(polls >= 3);
});

test("setStartAtLogin resolves the state read back, and subscribers see installed change alone", async () => {
  const install = loadFactory();
  let state = { state: "stopped-by-person", service: "launchd", installed: false, reachable: false };
  const { win, tick } = localHostWindow({
    local_host_state: () => state,
    local_host_action: ({ action }) => {
      state = { ...state, installed: action === "start-at-login-on" };
      return state;
    },
  });
  const local = install(win, ORIGIN, null, true).host.localHost;
  const seen = [];
  local.subscribe((s) => seen.push([s.state, s.installed]));
  await flush();
  assert.deepEqual(await local.setStartAtLogin(true), { ...state, installed: true });
  tick();
  await flush();
  assert.deepEqual(await local.setStartAtLogin(false), { ...state, installed: false });
  tick();
  await flush();
  assert.deepEqual(seen, [["stopped-by-person", false], ["stopped-by-person", true], ["stopped-by-person", false]]);
});

test("install: a promise of the state with its job, progress handed on when it changes, then no more polling", async () => {
  const install = loadFactory();
  let settle;
  const steps = [
    { step: "download", done: 0, total: 100, cancellable: true },
    { step: "download", done: 0, total: 100, cancellable: true },
    { step: "download", done: 60, total: null, cancellable: true },
    { step: "service-start", done: null, total: null, cancellable: false },
    null,
  ];
  const { win, calls } = localHostWindow({
    local_host_install: () => new Promise((resolve) => { settle = resolve; }),
    local_host_install_progress: () => (steps.length > 1 ? steps.shift() : steps[0]),
  });
  const local = install(win, ORIGIN, null, true).host.localHost;
  const seen = [];
  // Copied: the bridge makes its objects in its own realm.
  const operation = local.install((progress) => seen.push({ ...progress }));
  assert.equal(typeof operation.job, "string");
  assert.match(operation.job, /^local-install-1-/);
  assert.equal(typeof operation.then, "function");
  for (let i = 0; i < 20 && seen.length < 3; i++) await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(seen, [
    { step: "download", done: 0, total: 100, cancellable: true },
    { step: "download", done: 60, total: null, cancellable: true },
    { step: "service-start", done: null, total: null, cancellable: false },
  ], "unchanged progress is not handed on again; a null report is not progress");
  settle({ state: "running", reachable: true });
  assert.deepEqual(await operation, { state: "running", reachable: true });
  const polls = calls.filter(([cmd]) => cmd === "local_host_install_progress").length;
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(calls.filter(([cmd]) => cmd === "local_host_install_progress").length, polls, "no polling once settled");
  assert.deepEqual(calls.find(([cmd]) => cmd === "local_host_install"), ["local_host_install", { job: operation.job }]);
  assert.ok(calls.filter(([cmd]) => cmd === "local_host_install_progress").every(([, args]) => args.job === operation.job));
  assert.notEqual(local.install().job, operation.job, "each call its own job");
});

test("install rejects with the app's failure as it is; cancel goes by job and says whether it was honoured", async () => {
  const install = loadFactory();
  const busy = Object.assign(new Error("refused"), {
    refusal: { key: "install.busy", message: "Sidevoice is already being installed on this computer." },
  });
  const { win, calls } = localHostWindow({ local_host_install: busy, local_host_install_cancel: (args) => args.job === "j1" });
  const local = install(win, ORIGIN, null, true).host.localHost;
  const operation = local.install();
  await assert.rejects(operation, { key: "install.busy" });
  assert.equal(calls.filter(([cmd]) => cmd === "local_host_install_progress").length, 0, "no onProgress, no polling");
  assert.equal(await local.cancel("j1"), true);
  assert.equal(await local.cancel("j2"), false);
  for (const bad of [undefined, null, "", 3]) assert.equal(await local.cancel(bad), false, String(bad));
  assert.deepEqual(calls.filter(([cmd]) => cmd === "local_host_install_cancel"), [
    ["local_host_install_cancel", { job: "j1" }],
    ["local_host_install_cancel", { job: "j2" }],
  ]);
  const failed = Object.assign(new Error("refused"), {
    refusal: { key: "install.authenticity", step: "staging", params: { check: "archive-link" }, message: "m" },
  });
  const other = localHostWindow({ local_host_install: failed });
  await assert.rejects(install(other.win, ORIGIN, null, true).host.localHost.install(), {
    key: "install.authenticity", step: "staging", params: { check: "archive-link" },
  });
});

test("nothing of localHost runs until the page asks", async () => {
  const install = loadFactory();
  const { win, calls, tick } = localHostWindow({ local_host_state: { state: "running" } });
  install(win, ORIGIN, null, true);
  tick();
  await flush();
  assert.equal(calls.filter(([cmd]) => cmd.startsWith("local_host")).length, 0);
});

// ---- host.updater and host.build: the app's updates and what this build is (docs/BRIDGE.md) ----

test("host.build says what this build is, as the app passed it", () => {
  const install = loadFactory();
  const build = (given) => install(fakeWindow(ORIGIN).win, ORIGIN, null, false, false, false, given).host.build;
  assert.deepEqual({ ...build({ channel: "release", version: "0.4.0" }) }, { channel: "release", version: "0.4.0" });
  const nightly = { channel: "nightly", version: "0.4.1-nightly.3", commit: "a1b2c3d", date: "2026-10-13" };
  assert.deepEqual({ ...build(nightly) }, nightly);
  assert.ok(Object.isFrozen(build(nightly)));
});

test("the host's keys: updater only where the app can update itself, build always", () => {
  const install = loadFactory();
  // A dev build, a .deb, a bare binary: the app passes false, and the page shows nothing to update.
  for (const offered of [undefined, false, "__SIDEVOICE_UPDATER__", "true"]) {
    const api = install(fakeWindow(ORIGIN).win, ORIGIN, null, false, false, offered, { channel: "nightly", version: "0.4.0" });
    assert.deepEqual(Object.keys(api.host).sort(), ["app", "build", "callControls", "data", "engine", "mediaKeys", "microphone", "muteShortcut", "version"], String(offered));
    api.updaterEvent({ state: "idle" }); // nothing listens, nothing breaks
  }
  const api = install(fakeWindow(ORIGIN).win, ORIGIN, "native", true, true, true, { channel: "release", version: "0.4.0" });
  assert.deepEqual(Object.keys(api.host).sort(), ["app", "build", "callControls", "data", "engine", "localHost", "mediaKeys", "microphone", "muteShortcut", "updater", "version", "voice"]);
  // The AppUpdater seam, exactly.
  assert.deepEqual(Object.keys(api.host.updater).sort(), ["cancel", "check", "download", "restart", "setMode", "settings", "status", "subscribe"]);
  assert.ok(Object.isFrozen(api.host.updater));
});

test("host.computerName is the name the app passed, and absent without one", () => {
  const install = loadFactory();
  const host = (name) => install(fakeWindow(ORIGIN).win, ORIGIN, null, false, false, false, { channel: "dev", version: "0.4.0" }, name).host;
  assert.equal(host("Mac de Ana").computerName, "Mac de Ana");
  for (const none of [undefined, null, "", 42]) assert.ok(!("computerName" in host(none)), String(none));
});

test("host.updater's calls go to the updater command's actions and answer what the app says", async () => {
  const install = loadFactory();
  const answers = {
    settings: { mode: "automatic" },
    "set-mode": { mode: "manual" },
    status: { state: "downloading", version: "0.5.0", progress: 0.42 },
    check: { state: "available", version: "0.5.0" },
    download: { state: "downloading", version: "0.5.0" },
    restart: null,
    cancel: null,
  };
  const updaterWindow = (refuse) => {
    const { win, calls } = fakeWindow(ORIGIN);
    win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      calls.push([cmd, args === undefined ? undefined : structuredClone(args)]);
      if (cmd !== "updater") return Promise.resolve(null);
      return refuse?.[args.action] ? Promise.reject(refuse[args.action]) : Promise.resolve(answers[args.action]);
    };
    return { win, calls };
  };
  const { win, calls } = updaterWindow();
  const updater = install(win, ORIGIN, null, false, false, true).host.updater;
  assert.deepEqual(await updater.settings(), { mode: "automatic" });
  assert.deepEqual(await updater.setMode("manual"), { mode: "manual" });
  assert.deepEqual(await updater.status(), { state: "downloading", version: "0.5.0", progress: 0.42 });
  assert.deepEqual(await updater.check(), { state: "available", version: "0.5.0" });
  assert.deepEqual(await updater.download(), { state: "downloading", version: "0.5.0" });
  assert.equal(await updater.restart(), undefined);
  assert.equal(await updater.cancel(), undefined);
  assert.equal(await updater.cancel(), undefined, "with nothing to cancel it resolves too");
  assert.deepEqual(calls.filter(([cmd]) => cmd === "updater").map(([, args]) => args), [
    { action: "settings" },
    { action: "set-mode", mode: "manual" },
    { action: "status" },
    { action: "check" },
    { action: "download" },
    { action: "restart" },
    { action: "cancel" },
    { action: "cancel" },
  ]);
  const nothing = { key: "update_unavailable", message: "There is no update to install." };
  const refused = install(updaterWindow({ restart: nothing, download: nothing }).win, ORIGIN, null, false, false, true)
    .host.updater;
  await assert.rejects(refused.restart(), (e) => e === nothing);
  await assert.rejects(refused.download(), (e) => e === nothing);
});

test("the app's updaterEvent hands each listener the status, until it stops", () => {
  const install = loadFactory();
  const api = install(fakeWindow(ORIGIN).win, ORIGIN, null, false, false, true);
  const seen = [];
  const stop = api.host.updater.subscribe((status) => seen.push(["a", status.state]));
  api.host.updater.subscribe(() => { throw new Error("a listener's own"); });
  api.host.updater.subscribe((status) => seen.push(["b", status.state]));
  api.updaterEvent({ state: "downloading", version: "0.5.0", progress: 0.1 });
  assert.deepEqual(seen, [["a", "downloading"], ["b", "downloading"]], "every listener, past one that throws");
  stop();
  api.updaterEvent({ state: "ready", version: "0.5.0" });
  assert.deepEqual(seen, [["a", "downloading"], ["b", "downloading"], ["b", "ready"]]);
  assert.throws(() => api.host.updater.subscribe("nope"), { name: "TypeError" });
});

// ---- host.microphone: the app's microphone permission, for the page's first-run setup (docs/BRIDGE.md) ----

test("host.microphone is on every OS, with or without native voice; openSettings only where the OS has a page", async () => {
  const install = loadFactory();
  for (const voice of [false, true]) {
    const withSettings = install(fakeWindow(ORIGIN).win, ORIGIN, null, false, voice, false, undefined, undefined, undefined, true);
    assert.equal("voice" in withSettings.host, voice);
    assert.deepEqual(Object.keys(withSettings.host.microphone).sort(), ["onChange", "openSettings", "request", "state"]);
    assert.ok(Object.isFrozen(withSettings.host.microphone));
    for (const offered of [undefined, false, "true", "__SIDEVOICE_MICROPHONE_SETTINGS__"]) {
      const api = install(fakeWindow(ORIGIN).win, ORIGIN, null, false, voice, false, undefined, undefined, undefined, offered);
      assert.deepEqual(Object.keys(api.host.microphone).sort(), ["onChange", "request", "state"], "Linux: no settings page");
    }
  }
  const { win, calls } = fakeWindow(ORIGIN);
  const api = install(win, ORIGIN, null, false, false, false, undefined, undefined, undefined, true);
  assert.equal(await api.host.microphone.openSettings(), undefined);
  await api.host.microphone.state();
  await api.host.microphone.request();
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("microphone")), [
    ["microphone", { action: "open-settings" }],
    ["microphone", { action: "state" }],
    ["microphone", { action: "request" }],
  ], "without native voice, the same command: the OS answers, not the webview");
  assert.equal(install(fakeWindow("https://other.example").win, ORIGIN, null, false, false, false, undefined, undefined, undefined, true), null);
});

test("host.microphone calls the microphone command with its action, and passes refusals on", async () => {
  const install = loadFactory();
  let answer = "prompt";
  const { win, calls } = settingsWindow({
    microphone: ({ action }) => (action === "open-settings" ? null : answer),
  });
  const microphone = install(win, ORIGIN, "native", true, true, false, undefined, undefined, undefined, true).host.microphone;
  assert.equal(await microphone.state(), "prompt");
  answer = "granted";
  assert.equal(await microphone.request(), "granted");
  assert.equal(await microphone.openSettings(), undefined);
  assert.deepEqual(calls.filter(([cmd]) => cmd === "microphone"), [
    ["microphone", { action: "state" }],
    ["microphone", { action: "request" }],
    ["microphone", { action: "open-settings" }],
  ]);
  const refusal = { key: "internal", message: "System Settings could not be opened." };
  const failing = settingsWindow({ microphone: () => Object.assign(new Error(), { refusal }) });
  const host = install(failing.win, ORIGIN, "native", true, true, false, undefined, undefined, undefined, true).host;
  await assert.rejects(host.microphone.openSettings(), (error) => error === refusal);
});

test("host.microphone.onChange fires when state() changes (permission or device), until stopped", async () => {
  const install = loadFactory();
  let answer = "granted";
  const { win, tick } = settingsWindow({ microphone: () => answer });
  const microphone = install(win, ORIGIN, "native", true, true).host.microphone;
  assert.throws(() => microphone.onChange("nope"), { name: "TypeError" });
  let fired = 0;
  const stop = microphone.onChange(() => { fired++; });
  await flush(); // the first read is the baseline, not a change
  assert.equal(fired, 0);
  tick(); await flush();
  assert.equal(fired, 0);
  answer = "missing"; // unplugged
  tick(); await flush();
  assert.equal(fired, 1);
  answer = "prompt"; // plugged in, not asked yet
  tick(); await flush();
  assert.equal(fired, 2);
  stop();
  answer = "denied";
  tick(); await flush();
  assert.equal(fired, 2);
});

// ---- host.muteShortcut and host.callControls: the app's call settings, set from the page (docs/BRIDGE.md) ----

function settingsWindow(answers) {
  const fake = fakeWindow(ORIGIN);
  const { win, calls } = fake;
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, structuredClone(args)]);
    const answer = answers[cmd]?.(args);
    return answer instanceof Error ? Promise.reject(answer.refusal) : Promise.resolve(answer);
  };
  return { win, calls, tick: fake.tick };
}

test("host.muteShortcut and host.callControls are exactly the web's seams", () => {
  const install = loadFactory();
  const { host } = install(fakeWindow(ORIGIN).win, ORIGIN);
  assert.deepEqual(Object.keys(host.muteShortcut).sort(), ["get", "set", "subscribe"]);
  assert.deepEqual(Object.keys(host.callControls).sort(), ["clearSize", "endPreview", "preview", "resetPlace", "setAlwaysShown", "setCorner", "setEnabled", "setStageVisible", "settings"]);
  assert.ok(Object.isFrozen(host.muteShortcut) && Object.isFrozen(host.callControls));
});

test("host.muteShortcut gets and sets through the mute_shortcut command, refusing what is not a shortcut", async () => {
  const install = loadFactory();
  const on = { accelerator: "Cmd+Shift+M", label: "⇧⌘M", state: "on", alternatives: [], voiceOver: false };
  const off = { accelerator: null, label: null, state: "off", alternatives: [], voiceOver: false };
  const invalid = Object.assign(new Error(), { refusal: { key: "shortcut_invalid", message: "Not a shortcut: M" } });
  const { win, calls } = settingsWindow({
    mute_shortcut: ({ action, accelerator }) => action === "get" ? on : accelerator === null ? off : accelerator === "M" ? invalid : on,
  });
  const shortcut = install(win, ORIGIN).host.muteShortcut;
  assert.deepEqual(await shortcut.get(), on);
  assert.deepEqual(await shortcut.set("cmd+shift+m"), on);
  assert.deepEqual(await shortcut.set(null), off);
  await assert.rejects(shortcut.set("M"), (e) => e.key === "shortcut_invalid");
  for (const wrong of [undefined, 5, { accelerator: "Cmd+M" }]) {
    await assert.rejects(shortcut.set(wrong), (e) => e.key === "shortcut_invalid", String(wrong));
  }
  assert.deepEqual(calls.filter(([cmd]) => cmd === "mute_shortcut").map(([, args]) => args), [
    { action: "get" },
    { action: "set", accelerator: "cmd+shift+m" },
    { action: "set", accelerator: null },
    { action: "set", accelerator: "M" },
  ], "a wrong argument asks the app nothing");
});

test("the app's muteShortcutEvent reaches host.muteShortcut's listeners, until they stop", () => {
  const install = loadFactory();
  const api = install(fakeWindow(ORIGIN).win, ORIGIN);
  const seen = [];
  const stop = api.host.muteShortcut.subscribe((shortcut) => seen.push(shortcut.state));
  api.muteShortcutEvent({ accelerator: "Ctrl+Alt+M", label: "⌃⌥M", state: "refused", alternatives: ["Ctrl+Alt+Shift+M"], voiceOver: false });
  stop();
  api.muteShortcutEvent({ accelerator: null, label: null, state: "off", alternatives: [], voiceOver: false });
  assert.deepEqual(seen, ["refused"]);
});

test("an unavailable shortcut backend reaches get, set and subscription without losing the saved chord", async () => {
  const install = loadFactory();
  const unavailable = { accelerator: "Ctrl+Alt+M", label: "Ctrl+Alt+M", state: "unavailable", alternatives: [], voiceOver: false };
  const { win } = settingsWindow({ mute_shortcut: () => unavailable });
  const api = install(win, ORIGIN);
  assert.deepEqual(await api.host.muteShortcut.get(), unavailable);
  assert.deepEqual(await api.host.muteShortcut.set("Ctrl+Alt+M"), unavailable);
  const seen = [];
  const stop = api.host.muteShortcut.subscribe((shortcut) => seen.push(shortcut));
  api.muteShortcutEvent(unavailable);
  assert.deepEqual(JSON.parse(JSON.stringify(seen)), [unavailable]);
  stop();
});

test("host.callControls reads and sets alwaysShown through the call_controls_settings command, booleans only", async () => {
  const install = loadFactory();
  const { win, calls } = settingsWindow({
    call_controls_settings: ({ action, enabled }) => ({ alwaysShown: action === "settings" ? false : enabled }),
  });
  const controls = install(win, ORIGIN).host.callControls;
  assert.deepEqual(await controls.settings(), { alwaysShown: false });
  assert.deepEqual(await controls.setAlwaysShown(true), { alwaysShown: true });
  await assert.rejects(controls.setAlwaysShown("yes"), (e) => e.key === "bad_request");
  assert.deepEqual(calls.filter(([cmd]) => cmd === "call_controls_settings").map(([, args]) => args), [{ action: "settings" }, { action: "set-always-shown", enabled: true }]);
});

test("host.callControls turns the card off and on through the same command, booleans only", async () => {
  const install = loadFactory();
  const { win, calls } = settingsWindow({ call_controls_settings: ({ enabled }) => ({ enabled }) });
  const controls = install(win, ORIGIN).host.callControls;
  assert.deepEqual(await controls.setEnabled(false), { enabled: false });
  await assert.rejects(controls.setEnabled("no"), (e) => e.key === "bad_request");
  assert.deepEqual(calls.filter(([cmd]) => cmd === "call_controls_settings").map(([, args]) => args), [{ action: "set-enabled", enabled: false }]);
});

test("host.callControls sets the default corner, forgets places and previews, through the same command", async () => {
  const install = loadFactory();
  const { win, calls } = settingsWindow({ call_controls_settings: ({ action, corner }) => (action === "set-corner" ? { corner } : null) });
  const controls = install(win, ORIGIN).host.callControls;
  assert.deepEqual(await controls.setCorner("bl"), { corner: "bl" });
  await assert.rejects(controls.setCorner("middle"), (e) => e.key === "bad_request");
  assert.equal(await controls.resetPlace(), undefined);
  assert.equal(await controls.clearSize(), undefined);
  assert.equal(controls.preview(), undefined);
  await flush();
  assert.deepEqual(calls.filter(([cmd]) => cmd === "call_controls_settings").map(([, args]) => args),
    [{ action: "set-corner", corner: "bl" }, { action: "reset-place" }, { action: "clear-size" }, { action: "preview" }]);
});

test("host.callControls tells the app whether the page shows its stage, and where (#165)", async () => {
  const install = loadFactory();
  const { win, calls } = settingsWindow({ call_controls_settings: () => null });
  const controls = install(win, ORIGIN).host.callControls;
  assert.equal(controls.setStageVisible(true, { x: 260, y: 64, width: 800.5, height: 450, top: 64 }), undefined);
  controls.setStageVisible(false);
  controls.setStageVisible(true, { x: 1, y: NaN, width: 2, height: 3 });
  for (const wrong of [undefined, "yes", 1, null]) controls.setStageVisible(wrong);
  await flush();
  assert.deepEqual(calls.filter(([cmd]) => cmd === "call_controls_settings").map(([, args]) => args), [
    { action: "stage", visible: true, rect: { x: 260, y: 64, width: 800.5, height: 450 } },
    { action: "stage", visible: false },
    { action: "stage", visible: true },
  ], "only a boolean is sent, and a rect only when every side is a number");
});

// ---- the floating card's formats (sidevoice-web callcard contract) ----

test("the room's card projection reaches the app whole, facts included, with the bridge's own fields on top", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const facts = { ws: true, history: [{ thread: "a", role: "assistant", seq: 3, text: "hola" }], now: 5 };
  const rows = [{ threadId: "a", title: "mini", model: "opus", effort: "high" }];
  const store = fakeStore({ call: { joined: true }, mic: { enabled: true },
    callCard: { title: "mini", agent: "speaking", conversation: "a", mood: "speaking", turn: null, theme: "dark", language: "es",
      setupDone: true, busyReason: null, switchingTo: null, idleWarning: false, ended: null, micSilent: false, participants: rows, facts } });
  win.sidevoiceUI = { store };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {} };
  install(win, ORIGIN);
  await flush();
  const s = calls.filter(([cmd]) => cmd === "bridge_state").at(-1)[1].snapshot;
  assert.deepEqual(s.facts, facts);
  assert.deepEqual([s.conversation, s.mood, s.theme, s.language, s.setupDone, s.micSilent], ["a", "speaking", "dark", "es", true, false]);
  assert.deepEqual(s.participants, rows, "the room's own rows, as it projects them");
  assert.equal(s.version, 2);
  assert.ok(s.since > 0);
  const before = calls.length;
  store.set({ callCard: { ...store.getState().callCard, facts: { ...facts, karaokeState: { at: 2 } } } });
  await flush();
  assert.equal(calls.length, before + 1, "a change of the facts is a change");
});

test("the voice's output level reaches the app during a call only, throttled, and 0 when a reply ends", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  let publish = null;
  const store = fakeStore({ call: { joined: false }, mic: { enabled: true } });
  win.sidevoiceUI = { store, playbackLevel: { subscribe(fn) { publish = fn; return () => {}; } } };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {} };
  install(win, ORIGIN);
  publish({ level: 0.5, threadId: "a" });
  await flush();
  assert.equal(calls.filter(([cmd]) => cmd === "bridge_playback").length, 0, "no call, no level");
  store.set({ call: { joined: true } });
  publish({ level: 0.504, threadId: "a" });
  publish({ level: 0.7, threadId: "a" }); // within 40 ms of the last: dropped
  publish({ level: 0, threadId: "a" }); // the end of a reply always goes
  publish({ level: 0, threadId: "a" }); // no change
  await flush();
  assert.deepEqual(calls.filter(([cmd]) => cmd === "bridge_playback").map(([, args]) => args),
    [{ level: 0.5, threadId: "a" }, { level: 0, threadId: "a" }]);
});

test("the card's gear opens the room's Settings at the floating card, the tray's where it was left, and Cancel sending takes the turn back", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  const did = [];
  const store = fakeStore({ call: { joined: true }, mic: { enabled: true } });
  win.sidevoiceUI = { store, openSettings: (section) => did.push("settings " + section) };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {}, cancelInput: async () => did.push("cancel"), keepVoiceOn: () => did.push("keep") };
  const api = install(win, ORIGIN);
  assert.equal(api.run({ command: "open-settings" }), true);
  assert.equal(api.run({ command: "show-settings" }), true, "the tray's, with no section");
  assert.equal(api.run({ command: "cancel-input" }), true);
  assert.equal(api.run({ command: "keep-alive" }), true, "the idle warning's Keep");
  delete win.sidevoiceUI.openSettings;
  assert.equal(api.run({ command: "open-settings" }), false, "a room without it");
  assert.equal(api.run({ command: "show-settings" }), false, "a room without it");
  assert.deepEqual(did, ["settings floating-card", "settings undefined", "cancel", "keep"]);
});

// ---- host.debug: the developer options of a nightly or a dev build (docs/BRIDGE.md) ----

test("host.debug exists only where the app says the options (nightly and dev), and reads what the app kept", () => {
  const install = loadFactory();
  const host = (state) => install(fakeWindow(ORIGIN).win, ORIGIN, null, false, false, false, { channel: "nightly", version: "0.4.0" }, null, state).host;
  for (const none of [undefined, null, "__SIDEVOICE_DEBUG__", true, false, {}]) assert.ok(!("debug" in host(none)), String(none));
  const all = ["inspectable", "compositingBorders", "repaintCounters"];
  const debug = host({ available: all, inspectable: false, compositingBorders: true, repaintCounters: false }).debug;
  assert.deepEqual(Object.keys(debug).sort(), ["available", "compositingBorders", "inspectable", "repaintCounters", "set"]);
  assert.ok(Object.isFrozen(debug) && Object.isFrozen(debug.available));
  assert.deepEqual([...debug.available], all);
  assert.deepEqual([debug.inspectable, debug.compositingBorders, debug.repaintCounters], [false, true, false]);
  // A switch the platform lacks is not available and reads false.
  const none = host({ available: [], inspectable: true }).debug;
  assert.deepEqual([...none.available], []);
  assert.equal(none.inspectable, false);
});

test("host.debug.set goes to the debug_options command and each switch follows what the app took", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const kept = { inspectable: false, compositingBorders: false, repaintCounters: false };
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, plain(args)]);
    kept[args.name] = args.enabled;
    return Promise.resolve({ available: Object.keys(kept), ...kept });
  };
  const all = { available: ["inspectable", "compositingBorders", "repaintCounters"], ...kept };
  const debug = install(win, ORIGIN, null, false, false, false, { channel: "dev", version: "0.4.0" }, null, all).host.debug;
  assert.equal(await debug.set("inspectable", true), undefined);
  assert.equal(debug.inspectable, true);
  await debug.set("repaintCounters", true);
  assert.deepEqual([debug.inspectable, debug.compositingBorders, debug.repaintCounters], [true, false, true]);
  await debug.set("inspectable", false);
  assert.equal(debug.inspectable, false);
  assert.deepEqual(calls.filter(([cmd]) => cmd === "debug_options").map(([, args]) => args), [
    { action: "set", name: "inspectable", enabled: true },
    { action: "set", name: "repaintCounters", enabled: true },
    { action: "set", name: "inspectable", enabled: false },
  ]);
  for (const [name, on] of [["inspectable", "true"], ["inspectable", 1], ["inspectable", undefined], ["webInspector", true], [undefined, true]]) {
    await assert.rejects(debug.set(name, on), (e) => e.key === "bad_request", String([name, on]));
  }
  assert.equal(calls.filter(([cmd]) => cmd === "debug_options").length, 3);
  // Where the system cannot, the app answers false and the page sees it.
  win.__TAURI_INTERNALS__.invoke = () => Promise.resolve({ available: all.available, inspectable: false, compositingBorders: false, repaintCounters: false });
  await debug.set("compositingBorders", true);
  assert.equal(debug.compositingBorders, false);
});
