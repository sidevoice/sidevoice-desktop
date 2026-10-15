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

test("the macOS data bridge uses fixed native commands and no page-supplied paths", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const api = install(win, ORIGIN, null, true);
  assert.equal(typeof api.host.data.deleteAll, "function");
  assert.equal(typeof api.host.data.resetReport, "function");
  await api.host.data.pairings.read();
  await api.host.data.pairings.write({ pairings: [], pendingRevokes: [] });
  await api.host.data.deleteAll();
  await api.host.data.resetReport();
  assert.deepEqual(calls.slice(-4), [
    ["data_pairings_read", undefined],
    ["data_pairings_write", { value: { pairings: [], pendingRevokes: [] } }],
    ["data_delete_all", undefined],
    ["data_reset_report", undefined],
  ]);
  const other = fakeWindow(ORIGIN);
  const otherData = install(other.win, ORIGIN).host.data;
  assert.equal(typeof otherData.pairings.read, "function");
  assert.equal(typeof otherData.pairings.write, "function");
  assert.equal(otherData.deleteAll, undefined);
  assert.equal(otherData.resetReport, undefined);
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
    { threadId: "a", title: "mini", selected: true, reach: "listening", machine: "daimon", harness: "claude", working: false, subtitle: "", karaoke: "not relayed" },
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
  assert.deepEqual(plain(s.participants[0]), { threadId: "a", title: "mini", selected: true, reach: "listening", machine: "daimon", harness: "claude", working: false, subtitle: "" });
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
  assert.equal(api.run({ command: "select-participant", threadId: "c" }), false, "and one the call can go to");
  assert.equal(api.run({ command: "select-audio-device", kind: "input", id: "mbp" }), true);
  assert.equal(api.run({ command: "select-audio-device", kind: "output", id: "" }), true, "the system's default");
  assert.equal(api.run({ command: "select-audio-device", kind: "camera", id: "x" }), false);
  assert.equal(api.run({ command: "select-audio-device", kind: "input", id: 3 }), false);
  await flush();
  assert.deepEqual(did, ["skip", "select b", 'devices {"input":"mbp"}', 'devices {"output":null}']);
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

test("host.voice only where the app runs the call, and exactly the seam", () => {
  const install = loadFactory();
  for (const offered of [undefined, false, "__SIDEVOICE_VOICE__", "true"]) {
    const api = install(fakeWindow(ORIGIN).win, ORIGIN, null, true, offered);
    assert.equal("voice" in api.host, false, String(offered));
    api.voiceEvent({ type: "level", data: 0.5 }); // nothing listens, nothing breaks
  }
  const api = install(fakeWindow(ORIGIN).win, ORIGIN, "native", true, true);
  assert.deepEqual(Object.keys(api.host.voice).sort(), [
    "audioDevices", "cancelInput", "mute", "onAgentInterrupted", "onAudioDevices", "onError", "onLevel", "onPlaybackLevel", "onState",
    "onTake", "onTurn", "say",
    "setAudioDevices", "setCaptureBoundary", "setSettings", "start", "stop",
  ]);
  assert.ok(Object.isFrozen(api.host.voice));
});

test("host.voice calls go to their native commands, with their arguments", async () => {
  const install = loadFactory();
  const { win, calls } = voiceWindow();
  const voice = install(win, ORIGIN, "native", true, true).host.voice;
  const settings = {
    stt: { catalog: "local", model: "whisper-small", language: "es" },
    tts: { catalog: "local", model: "kokoro-82m-v1.0", voice: "ef_dora", speed: 1 },
  };
  await voice.setCaptureBoundary("focus-a");
  await voice.setSettings(settings);
  await voice.start();
  voice.say("Hola.", { language: "es" });
  voice.say("Sin idioma.");
  await voice.mute(1);
  await voice.cancelInput();
  await voice.setCaptureBoundary("focus-b");
  await voice.stop();
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("voice_")), [
    ["voice_set_capture_boundary", { id: "focus-a" }],
    ["voice_set_settings", { settings }],
    ["voice_start", { take: false }],
    ["voice_say", { key: "say-1", text: "Hola.", language: "es" }],
    ["voice_say", { key: "say-2", text: "Sin idioma.", language: null }],
    ["voice_mute", { muted: true }],
    ["voice_cancel_input", undefined],
    ["voice_set_capture_boundary", { id: "focus-b" }],
    ["voice_stop", undefined],
  ]);
});

test("something said answers its handle at once: its steps, its outcome, and a cancel that never overtakes the say", async () => {
  const install = loadFactory();
  const { win, calls } = voiceWindow();
  const api = install(win, ORIGIN, "native", true, true);
  const saying = api.host.voice.say("Hecho, ya está en la rama.");
  assert.equal(saying.id, "say-1");
  assert.ok(Object.isFrozen(saying));
  const steps = [];
  saying.onEvent((step) => steps.push(step.type));
  saying.onEvent(() => { throw new Error("a listener's own"); });
  saying.cancel();
  await new Promise((resolve) => setTimeout(resolve, 0));
  const voiceCalls = () => calls.filter(([cmd]) => cmd.startsWith("voice_"));
  assert.deepEqual(voiceCalls().map(([cmd]) => cmd), ["voice_say", "voice_cancel_say"], "the cancel goes behind its say");
  assert.deepEqual(voiceCalls()[1][1], { key: "say-1" });
  const step = (event) => api.voiceEvent({ type: "say", data: { key: "say-1", event } });
  step({ type: "playing" });
  step({ type: "progress", sounding: [0, 26], heard_chars: 0 });
  const outcome = { status: "heard-up-to", heard_chars: 0, reason: "cancelled" };
  step({ type: "done", outcome });
  assert.deepEqual(await saying.outcome, outcome);
  assert.deepEqual(steps, ["playing", "progress", "done"]);
  // Done: a late step goes nowhere, and a cancel then is nothing.
  step({ type: "playing" });
  assert.equal(steps.length, 3);
  saying.cancel();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(voiceCalls().length, 2);
  // A say the app refuses ends as not played, failed with the refusal's code.
  const refusal = { key: "bad_request", code: "bad-request", message: "not a say key" };
  const refused = install(voiceWindow({ voice_say: Object.assign(new Error(), { refusal }) }).win, ORIGIN, "native", true, true)
    .host.voice.say("x");
  assert.equal(JSON.stringify(await refused.outcome), '{"status":"not-played","reason":"failed","code":"bad-request"}');
});

test("candidate hearing policy consumes native physical outcomes without a protocol skip cause", {
  skip: !process.env.SIDEVOICE_CANDIDATE_WEB_SOURCE,
}, async () => {
  const services = path.join(process.env.SIDEVOICE_CANDIDATE_WEB_SOURCE, "apps/web/src/services");
  const voiceSource = readFileSync(path.join(services, "voice-room.js"), "utf8");
  const policySource = readFileSync(path.join(services, "reply-hearing-policy.js"), "utf8");
  const { playbackReport } = await import(`data:text/javascript,${encodeURIComponent(voiceSource)}`);
  const { playbackHearing } = await import(`data:text/javascript,${encodeURIComponent(policySource)}`);
  const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
  const text = "é🐙ab";
  async function nativeOutcome(outcome) {
    const saying = api.host.voice.say(text);
    let done;
    saying.onEvent((step) => { if (step.type === "done") done = plain(step.outcome); });
    api.voiceEvent({ type: "say", data: { key: saying.id, event: { type: "done", outcome } } });
    const promised = plain(await saying.outcome);
    assert.deepEqual(done, promised, "native done event and promise have the same flat host outcome");
    return promised;
  }

  const skipped = await nativeOutcome({ status: "heard-up-to", heard_chars: 2, reason: { reason: "cancelled" } });
  const skipPlayback = playbackReport(skipped, text, null, "user_skipped");
  assert.deepEqual(skipPlayback, { status: "interrupted", heard_chars: 2 });
  assert.deepEqual(playbackHearing(skipPlayback, text, "user_skipped"), { outcome: "heard" });

  const interrupted = await nativeOutcome({ status: "heard-up-to", heard_chars: 2, reason: { reason: "barge-in" } });
  const cutPlayback = playbackReport(interrupted, text);
  assert.deepEqual(cutPlayback, { status: "interrupted", heard_chars: 2, reason: "user_interrupted" });
  assert.deepEqual(playbackHearing(cutPlayback, text), { outcome: "partial", heard_chars: 2 });

  const unplayed = await nativeOutcome({ status: "not-played", reason: { reason: "cancelled" } });
  assert.deepEqual(playbackHearing(playbackReport(unplayed, text), text), { outcome: "not_heard" });
});

test("the call's events reach host.voice's listeners by their type, until they stop", () => {
  const install = loadFactory();
  const api = install(voiceWindow().win, ORIGIN, "native", true, true);
  const seen = [];
  const listen = (name) => api.host.voice[name]((data) => seen.push([name, JSON.stringify(data)]));
  const stops = ["onTurn", "onState", "onLevel", "onError"].map(listen);
  api.host.voice.onLevel(() => { throw new Error("a listener's own"); });
  const turn = { phase: "started", turn_id: "c-turn-0", capture_boundary_id: "focus-a", started_at: 1 };
  api.voiceEvent({ type: "turn", data: turn });
  api.voiceEvent({ type: "state", data: { listening: "listening" } });
  api.voiceEvent({ type: "level", data: 0.25 });
  api.voiceEvent({ type: "error", data: { code: "microphone-denied" } });
  api.voiceEvent({ type: "say", data: { key: "nobody", event: { type: "playing" } } });
  api.voiceEvent({ type: "something-else", data: {} });
  api.voiceEvent(null);
  assert.deepEqual(seen.map(([name]) => name), ["onTurn", "onState", "onLevel", "onError"]);
  assert.equal(seen[0][1], JSON.stringify(turn), "the call's turn, as it told it");
  stops.forEach((stop) => stop());
  api.voiceEvent({ type: "level", data: 0.5 });
  assert.equal(seen.length, 4);
  assert.throws(() => api.host.voice.onState("not a function"), { name: "TypeError" });
});

test("A to B focus fences preserve captured routing through the native host seam", async () => {
  const { win, calls } = voiceWindow();
  let release;
  win.__TAURI_INTERNALS__.invoke = (command, args) => {
    calls.push([command, args === undefined ? undefined : structuredClone(args)]);
    if (command === "voice_set_capture_boundary" && args.id === "stt-b") {
      return new Promise((resolve) => { release = resolve; });
    }
    return Promise.resolve();
  };
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const voice = api.host.voice;
  const captured = new Map();
  const routed = [];
  let focus = "A";
  voice.onTurn((turn) => {
    if (turn.phase === "started") {
      captured.set(turn.turn_id, { focus, boundary: turn.capture_boundary_id });
    } else if (turn.phase === "finished") {
      const original = captured.get(turn.turn_id);
      assert.equal(turn.capture_boundary_id, original.boundary);
      assert.equal(turn.merged, false, "cross-focus words never merge");
      routed.push([original.focus, turn.text]);
    }
  });
  const emit = (phase, turn_id, capture_boundary_id, text) => api.voiceEvent({ type: "turn", data: {
    phase, turn_id, capture_boundary_id,
    ...(phase === "started" ? { started_at: 1 } : {
      text, language: "en", started_at: 1, ended_at: 2, merged: false,
      timings: { audio_ms: 1, endpoint_silence_ms: 0, recognition_ms: 1 },
    }),
  } });

  // A is in STT. The new focus waits for the native call to acknowledge its fence; A may finish after B starts.
  await voice.setCaptureBoundary("stt-a");
  emit("started", "a-stt", "stt-a");
  let acknowledged = false;
  const boundary = voice.setCaptureBoundary("stt-b").then(() => { acknowledged = true; });
  await flush();
  assert.equal(acknowledged, false);
  release();
  await boundary;
  focus = "B";
  emit("started", "b-stt", "stt-b");
  emit("finished", "a-stt", "stt-a", "Private instruction for A.");
  emit("finished", "b-stt", "stt-b", "Instruction for B.");
  assert.deepEqual(routed, [["A", "Private instruction for A."], ["B", "Instruction for B."]]);

  // A's held transcript is released by the fence, before any B turn begins.
  routed.length = 0;
  focus = "A";
  await voice.setCaptureBoundary("held-a");
  emit("started", "a-held", "held-a");
  await voice.setCaptureBoundary("held-b");
  emit("finished", "a-held", "held-a", "Held words for A.");
  focus = "B";
  emit("started", "b-held", "held-b");
  emit("finished", "b-held", "held-b", "Words for B.");
  assert.deepEqual(routed, [["A", "Held words for A."], ["B", "Words for B."]]);
  assert.deepEqual(calls.filter(([command]) => command === "voice_set_capture_boundary"), [
    ["voice_set_capture_boundary", { id: "stt-a" }],
    ["voice_set_capture_boundary", { id: "stt-b" }],
    ["voice_set_capture_boundary", { id: "held-a" }],
    ["voice_set_capture_boundary", { id: "held-b" }],
  ]);
  await assert.rejects(voice.setCaptureBoundary(""), { name: "TypeError" });
});

test("R7 delayed native start keeps A's boundary after focus and binding replacement", async () => {
  const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
  const voice = api.host.voice;
  const pending = [];
  const seen = [];
  voice.onTurn((turn) => seen.push(plain(turn)));
  const queueNative = (data) => pending.push(JSON.stringify({ type: "turn", data }));
  const deliver = () => api.voiceEvent(JSON.parse(pending.shift()));

  await voice.setCaptureBoundary("capture-a-old");
  queueNative({ phase: "started", turn_id: "a-turn", capture_boundary_id: "capture-a-old", started_at: 1 });
  queueNative({ phase: "started", turn_id: "a-cancel", capture_boundary_id: "capture-a-old", started_at: 1 });
  // Native has observed A's start, but its webview.eval has not yet delivered it to the page.
  await voice.setCaptureBoundary("capture-b");
  const currentFocus = { thread_id: "b", binding_id: "binding-b" };
  const replacementForA = { thread_id: "a", binding_id: "binding-a-new" };
  assert.equal(currentFocus.thread_id, "b");
  assert.notEqual(replacementForA.binding_id, "binding-a-old");
  deliver();
  deliver();
  queueNative({ phase: "started", turn_id: "b-turn", capture_boundary_id: "capture-b", started_at: 2 });
  deliver();
  // The two STT completions may arrive in reverse order; neither phase is rebound to current focus.
  queueNative({ phase: "finished", turn_id: "b-turn", capture_boundary_id: "capture-b", text: "Words for B.",
    started_at: 2, ended_at: 3, merged: false, timings: { audio_ms: 1, endpoint_silence_ms: 0, recognition_ms: 1 } });
  queueNative({ phase: "finished", turn_id: "a-turn", capture_boundary_id: "capture-a-old", text: "Private words for A.",
    started_at: 1, ended_at: 4, merged: false, timings: { audio_ms: 1, endpoint_silence_ms: 0, recognition_ms: 1 } });
  queueNative({ phase: "cancelled", turn_id: "a-cancel", capture_boundary_id: "capture-a-old", merged: false });
  deliver();
  deliver();
  deliver();
  assert.deepEqual(seen.map(({ phase, turn_id, capture_boundary_id }) => [phase, turn_id, capture_boundary_id]), [
    ["started", "a-turn", "capture-a-old"], ["started", "a-cancel", "capture-a-old"],
    ["started", "b-turn", "capture-b"], ["finished", "b-turn", "capture-b"],
    ["finished", "a-turn", "capture-a-old"], ["cancelled", "a-cancel", "capture-a-old"],
  ]);
  assert.deepEqual(seen.filter((turn) => turn.phase === "finished").map(({ turn_id, text }) => [turn_id, text]), [
    ["b-turn", "Words for B."], ["a-turn", "Private words for A."],
  ]);
});

test("R7 candidate web frames admit delayed native starts to their captured core targets", {
  skip: !process.env.SIDEVOICE_CANDIDATE_WEB_SOURCE,
}, async () => {
  const source = readFileSync(path.join(process.env.SIDEVOICE_CANDIDATE_WEB_SOURCE,
    "apps/web/src/services/voice-room.js"), "utf8");
  const { turnMessage } = await import(`data:text/javascript,${encodeURIComponent(source)}`);
  const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
  const voice = api.host.voice;
  const targets = new Map([
    ["capture-a-old", { thread_id: "a", binding_id: "binding-a-old" }],
    ["capture-b", { thread_id: "b", binding_id: "binding-b" }],
  ]);
  const admitted = new Map();
  const epochs = { a: 0, b: 0 };
  const queues = { a: [], b: [] };
  let current = { thread_id: "a", binding_id: "binding-a-old" };
  const recentFocusPairs = new Set(["a:binding-a-old", "a:binding-a-new", "b:binding-b"]);
  const frames = [];
  voice.onTurn((turn) => {
    const target = targets.get(turn.capture_boundary_id);
    assert.ok(target, "every native turn names its source capture boundary");
    const frame = turnMessage(turn, target);
    frames.push(plain(frame));
    if (frame.phase === "started") {
      // Model core's admission point. The old producer omitted this pair and admitted against current B.
      const admittedTarget = frame.thread_id && frame.binding_id
        ? { thread_id: frame.thread_id, binding_id: frame.binding_id } : current;
      assert.ok(recentFocusPairs.has(`${admittedTarget.thread_id}:${admittedTarget.binding_id}`));
      admitted.set(frame.turn_id, { ...admittedTarget });
      epochs[admittedTarget.thread_id] += 1;
    } else if (frame.phase === "finished") {
      const bound = admitted.get(frame.turn_id);
      assert.ok(bound, "a terminal requires its admitted start");
      queues[bound.thread_id].push(frame.text);
    }
  });
  const pending = [];
  const native = (data) => pending.push(JSON.stringify({ type: "turn", data }));
  const deliver = () => api.voiceEvent(JSON.parse(pending.shift()));
  await voice.setCaptureBoundary("capture-a-old");
  native({ phase: "started", turn_id: "a-turn", capture_boundary_id: "capture-a-old", started_at: 1 });
  current = { thread_id: "b", binding_id: "binding-b" };
  await voice.setCaptureBoundary("capture-b");
  // A's binding was replaced while the native start was in flight. Its old captured pair remains valid.
  const replacementForA = { thread_id: "a", binding_id: "binding-a-new" };
  assert.notEqual(targets.get("capture-a-old").binding_id, replacementForA.binding_id);
  assert.equal(current.thread_id, "b");
  deliver();
  native({ phase: "started", turn_id: "b-turn", capture_boundary_id: "capture-b", started_at: 2 });
  deliver();
  native({ phase: "finished", turn_id: "b-turn", capture_boundary_id: "capture-b", text: "Words for B.",
    started_at: 2, ended_at: 3, merged: false, timings: { audio_ms: 1, endpoint_silence_ms: 0, recognition_ms: 1 } });
  native({ phase: "finished", turn_id: "a-turn", capture_boundary_id: "capture-a-old", text: "Private words for A.",
    started_at: 1, ended_at: 4, merged: false, timings: { audio_ms: 1, endpoint_silence_ms: 0, recognition_ms: 1 } });
  deliver();
  deliver();
  assert.deepEqual([frames[0].thread_id, frames[0].binding_id], ["a", "binding-a-old"]);
  assert.deepEqual([frames[1].thread_id, frames[1].binding_id], ["b", "binding-b"]);
  assert.deepEqual(epochs, { a: 1, b: 1 });
  assert.deepEqual(queues, { a: ["Private words for A."], b: ["Words for B."] });
});

test("playback level forwards bare numbers and zero independently of microphone level", () => {
  const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
  const microphone = [];
  const output = [];
  const other = [];
  const stopMic = api.host.voice.onLevel((level) => microphone.push(level));
  const stopOutput = api.host.voice.onPlaybackLevel((level) => output.push(level));
  api.host.voice.onPlaybackLevel(() => { throw new Error("a listener's own"); });
  api.host.voice.onPlaybackLevel((level) => other.push(level));
  api.voiceEvent({ type: "playback-level", data: 0.7 });
  api.voiceEvent({ type: "level", data: 0.25 });
  api.voiceEvent({ type: "playback-level", data: 0 });
  assert.deepEqual(output, [0.7, 0]);
  assert.deepEqual(other, [0.7, 0]);
  assert.deepEqual(microphone, [0.25]);
  stopOutput();
  stopOutput();
  api.voiceEvent({ type: "playback-level", data: 0.4 });
  assert.deepEqual(output, [0.7, 0]);
  assert.deepEqual(other, [0.7, 0, 0.4]);
  stopMic();
  assert.throws(() => api.host.voice.onPlaybackLevel(null), { name: "TypeError" });
});

test("T01/T04b: native cut ids reach the matching played and queued say handles with scalar extents", async () => {
  const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
  const first = api.host.voice.say("😀reply one");
  const second = api.host.voice.say("reply two");
  const order = [];
  const stop = api.host.voice.onAgentInterrupted((cut) => order.push(["cut", cut]));
  first.onEvent((step) => order.push(["first", step]));
  second.onEvent((step) => order.push(["second", step]));
  const firstCut = { utterance_id: first.id, heard_until: 1, played: true };
  const queuedCut = { utterance_id: second.id, heard_until: 0, played: false };
  api.voiceEvent({ type: "agent-interrupted", data: firstCut });
  api.voiceEvent({ type: "say", data: { key: first.id, event: { type: "done", outcome: { status: "heard-up-to", heard_chars: 1, reason: { reason: "barge-in" } } } } });
  api.voiceEvent({ type: "agent-interrupted", data: queuedCut });
  api.voiceEvent({ type: "say", data: { key: second.id, event: { type: "done", outcome: { status: "not-played", reason: { reason: "barge-in" } } } } });
  assert.deepEqual(order.map(([kind]) => kind), ["cut", "first", "cut", "second"]);
  assert.deepEqual(order.filter(([kind]) => kind === "cut").map(([, cut]) => cut), [firstCut, queuedCut]);
  assert.deepEqual(plain(await first.outcome), { status: "heard-up-to", heard_chars: 1, reason: "barge-in" });
  assert.deepEqual(plain(await second.outcome), { status: "not-played", reason: "barge-in" });
  stop();
  api.voiceEvent({ type: "agent-interrupted", data: firstCut });
  assert.equal(order.length, 4);
});

for (const [caseId, terminal] of [
  ["T02a", { phase: "finished", turn_id: "user", text: "new words", started_at: 10, ended_at: 1200, merged: false }],
  ["T02b", { phase: "cancelled", turn_id: "user", merged: false }],
]) {
  test(`${caseId}: accepted but unsounded reply stays unplayed through the turn terminal`, async () => {
    const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
    const say = api.host.voice.say("accepted first");
    const seen = [];
    api.host.voice.onTurn((turn) => seen.push(turn.phase));
    api.host.voice.onAgentInterrupted((cut) => seen.push(cut));
    say.onEvent((event) => seen.push(event));
    api.voiceEvent({ type: "turn", data: { phase: "started", turn_id: "user", started_at: 10 } });
    api.voiceEvent({ type: "agent-interrupted", data: { utterance_id: say.id, heard_until: 0, played: false } });
    api.voiceEvent({ type: "say", data: { key: say.id, event: { type: "done", outcome: { status: "not-played", reason: { reason: "barge-in" } } } } });
    api.voiceEvent({ type: "turn", data: terminal });
    assert.deepEqual(seen.map((step) => typeof step === "string" ? step : step.played === false ? "cut" : step.type || step.phase),
      ["started", "cut", "done", terminal.phase]);
    assert.deepEqual(plain(await say.outcome), { status: "not-played", reason: "barge-in" });
  });
}

test("T05: an acknowledgement, checkpoint and result keep distinct say handles and checkpoint cut extent", async () => {
  const { win, calls } = voiceWindow();
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const sayings = ["ack", "checkpoint", "result"].map((text) => api.host.voice.say(text));
  const cuts = [];
  api.host.voice.onAgentInterrupted((cut) => cuts.push(cut));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_say").map(([, args]) => args.key), sayings.map((say) => say.id));
  api.voiceEvent({ type: "say", data: { key: sayings[0].id, event: { type: "done", outcome: { status: "heard" } } } });
  api.voiceEvent({ type: "agent-interrupted", data: { utterance_id: sayings[1].id, heard_until: 3, played: true } });
  api.voiceEvent({ type: "say", data: { key: sayings[1].id, event: { type: "done", outcome: { status: "heard-up-to", heard_chars: 3, reason: { reason: "barge-in" } } } } });
  api.voiceEvent({ type: "agent-interrupted", data: { utterance_id: sayings[2].id, heard_until: 0, played: false } });
  api.voiceEvent({ type: "say", data: { key: sayings[2].id, event: { type: "done", outcome: { status: "not-played", reason: { reason: "barge-in" } } } } });
  assert.deepEqual(plain(await Promise.all(sayings.map((say) => say.outcome))), [
    { status: "heard" },
    { status: "heard-up-to", heard_chars: 3, reason: "barge-in" },
    { status: "not-played", reason: "barge-in" },
  ]);
  assert.deepEqual(cuts.map((cut) => [cut.utterance_id, cut.heard_until]), [[sayings[1].id, 3], [sayings[2].id, 0]]);
});

test("T04a/T12: cancelling a sounding reply sends only its key and preserves its queued successor", async () => {
  const { win, calls } = voiceWindow();
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const first = api.host.voice.say("first");
  const next = api.host.voice.say("next");
  api.voiceEvent({ type: "say", data: { key: first.id, event: { type: "playing" } } });
  first.cancel();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_say" || cmd === "voice_cancel_say").map(([cmd, args]) => [cmd, args.key]), [
    ["voice_say", first.id], ["voice_say", next.id], ["voice_cancel_say", first.id],
  ]);
  api.voiceEvent({ type: "say", data: { key: first.id, event: { type: "done", outcome: { status: "heard-up-to", heard_chars: 2, reason: { reason: "cancelled" } } } } });
  api.voiceEvent({ type: "say", data: { key: next.id, event: { type: "playing" } } });
  api.voiceEvent({ type: "say", data: { key: next.id, event: { type: "done", outcome: { status: "heard" } } } });
  assert.equal((await first.outcome).heard_chars, 2);
  assert.deepEqual(await next.outcome, { status: "heard" });
});

test("T02/T09b/T13: mute forwards silence semantics and preserves turn start, terminal and playback", async () => {
  const { win, calls } = voiceWindow();
  const api = loadFactory()(win, ORIGIN, "native", true, true);
  const seen = [];
  api.host.voice.onTurn((turn) => seen.push(turn));
  api.host.voice.onPlaybackLevel((level) => seen.push(level));
  api.voiceEvent({ type: "turn", data: { phase: "started", turn_id: "u1", started_at: 100 } });
  await api.host.voice.mute(true);
  api.voiceEvent({ type: "playback-level", data: 0.6 });
  api.voiceEvent({ type: "turn", data: { phase: "finished", turn_id: "u1", text: "yes", started_at: 100, ended_at: 1200, merged: false } });
  api.voiceEvent({ type: "turn", data: { phase: "started", turn_id: "u2", started_at: 1300 } });
  api.voiceEvent({ type: "turn", data: { phase: "cancelled", turn_id: "u2", merged: true } });
  assert.deepEqual(calls.filter(([cmd]) => cmd === "voice_mute"), [["voice_mute", { muted: true }]]);
  assert.deepEqual(seen.map((event) => event.phase || event), ["started", 0.6, "finished", "started", "cancelled"]);
});

test("T07: speaker events and output loss are forwarded without inventing a turn or replay", () => {
  const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
  const turns = [];
  const devices = [];
  api.host.voice.onTurn((turn) => turns.push(turn));
  api.host.voice.onAudioDevices((device) => devices.push(device));
  api.voiceEvent({ type: "playback-level", data: 0.8 });
  api.voiceEvent({ type: "devices", data: { inputs: [], outputs: [], input: null, output: null, outputSelectable: true, lost: { output: "old" } } });
  api.voiceEvent({ type: "playback-level", data: 0 });
  assert.deepEqual(turns, []);
  assert.deepEqual(devices.map((device) => device.lost), [{ output: "old" }]);
});

test("native stop-reason JSON becomes the flat VoiceHost outcome, including a failure code", async () => {
  const api = loadFactory()(voiceWindow().win, ORIGIN, "native", true, true);
  const say = api.host.voice.say("check");
  const steps = [];
  say.onEvent((event) => steps.push(event));
  api.voiceEvent({ type: "say", data: { key: say.id, event: {
    type: "done", outcome: { status: "heard-up-to", heard_chars: 0, reason: { reason: "failed", code: "speech-failed" } },
  } } });
  const expected = { status: "heard-up-to", heard_chars: 0, reason: "failed", code: "speech-failed" };
  assert.deepEqual(plain(steps), [{ type: "done", outcome: expected }]);
  assert.deepEqual(plain(await say.outcome), expected);
});

test("the audio devices: listed, chosen by kind (null the default, a missing kind as it is), told when they change", async () => {
  const install = loadFactory();
  const devices = {
    inputs: [{ id: "CoreAudio:mic", label: "MacBook Pro Microphone" }],
    outputs: [{ id: "CoreAudio:speakers", label: "MacBook Pro Speakers" }],
    input: null, output: "CoreAudio:speakers", outputSelectable: true,
  };
  const { win, calls } = voiceWindow({ voice_audio_devices: devices, voice_set_audio_devices: devices });
  const api = install(win, ORIGIN, "native", true, true);
  const voice = api.host.voice;
  assert.deepEqual(await voice.audioDevices(), devices);
  assert.deepEqual(await voice.setAudioDevices({ output: "CoreAudio:speakers" }), devices);
  await voice.setAudioDevices({ input: null, output: undefined, extra: "dropped" });
  await voice.setAudioDevices({ input: "CoreAudio:mic", output: null });
  assert.deepEqual(calls.filter(([cmd]) => cmd.includes("audio_devices")), [
    ["voice_audio_devices", undefined],
    ["voice_set_audio_devices", { choice: { output: "CoreAudio:speakers" } }],
    ["voice_set_audio_devices", { choice: { input: null } }],
    ["voice_set_audio_devices", { choice: { input: "CoreAudio:mic", output: null } }],
  ]);
  const seen = [];
  const stop = voice.onAudioDevices((listed) => seen.push(listed));
  const lost = { ...devices, output: null, lost: { output: "CoreAudio:speakers" } };
  api.voiceEvent({ type: "devices", data: lost });
  stop();
  api.voiceEvent({ type: "devices", data: devices });
  assert.deepEqual(seen, [lost]);
  // An id the system does not list: the app's refusal, as it is.
  const refusal = { key: "voice_audio_device_unknown", code: "audio-device-unknown", message: "The audio devices: audio-device-unknown." };
  const refusing = install(voiceWindow({ voice_set_audio_devices: Object.assign(new Error(), { refusal }) }).win, ORIGIN, null, true, true);
  await assert.rejects(refusing.host.voice.setAudioDevices({ input: "gone" }), (error) => error === refusal);
});

test("a refused start rejects with the app's keyed refusal", async () => {
  const install = loadFactory();
  const refusal = { key: "voice_failed", code: "microphone-denied", message: "The call could not start: microphone-denied." };
  const voice = install(voiceWindow({ voice_start: Object.assign(new Error(), { refusal }) }).win, ORIGIN, null, true, true)
    .host.voice;
  await assert.rejects(voice.start(), (error) => error === refusal);
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
    "cancel", "install", "pairRoom", "pairing", "pairingCode", "reconnect", "restart", "revealLog", "serviceInstall",
    "serviceUninstall", "setStartAtLogin", "start", "state", "stop", "subscribe",
  ]);
  assert.ok(Object.isFrozen(api.host.localHost));
});

test("localHost calls go to their native commands and resolve what the app answers", async () => {
  const install = loadFactory();
  const pairing = { fp: "fp", public_key: "k", device_id: "d", token: "secret", urls: ["http://127.0.0.1:5"], rv: null, host: "Mac", local: true };
  const { win, calls } = localHostWindow({
    local_host_state: { state: "running", service: "launchd", calls: 0 },
    local_host_pairing: pairing,
    local_host_action: (args) => ({ state: args.action === "stop" ? "stopped-by-person" : "running" }),
    local_host_pairing_code: { code: "SV1", expires_in: 600, reach: "room" },
    local_host_pair_room: { room: "https://room.example" },
  });
  const local = install(win, ORIGIN, null, true).host.localHost;
  assert.deepEqual(await local.state(), { state: "running", service: "launchd", calls: 0 });
  assert.deepEqual(await local.pairing(), pairing);
  assert.deepEqual(await local.stop(), { state: "stopped-by-person" });
  for (const name of ["start", "restart", "serviceInstall", "serviceUninstall", "reconnect", "revealLog"]) await local[name]();
  await local.setStartAtLogin(true);
  await local.setStartAtLogin(false);
  assert.deepEqual(await local.pairingCode(), { code: "SV1", expires_in: 600, reach: "room" });
  assert.deepEqual(await local.pairRoom("https://room.example", "CODE"), { room: "https://room.example" });
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
    ["local_host_pair_room", { url: "https://room.example", code: "CODE" }],
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
    assert.deepEqual(Object.keys(api.host).sort(), ["app", "build", "callControls", "data", "engine", "mediaKeys", "muteShortcut", "version"], String(offered));
    api.updaterEvent({ state: "idle" }); // nothing listens, nothing breaks
  }
  const api = install(fakeWindow(ORIGIN).win, ORIGIN, "native", true, true, true, { channel: "release", version: "0.4.0" });
  assert.deepEqual(Object.keys(api.host).sort(), ["app", "build", "callControls", "data", "engine", "localHost", "mediaKeys", "muteShortcut", "updater", "version", "voice"]);
  // The AppUpdater seam, exactly.
  assert.deepEqual(Object.keys(api.host.updater).sort(), ["check", "download", "restart", "setMode", "settings", "status", "subscribe"]);
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
  assert.deepEqual(calls.filter(([cmd]) => cmd === "updater").map(([, args]) => args), [
    { action: "settings" },
    { action: "set-mode", mode: "manual" },
    { action: "status" },
    { action: "check" },
    { action: "download" },
    { action: "restart" },
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

// ---- host.muteShortcut and host.callControls: the app's call settings, set from the page (docs/BRIDGE.md) ----

function settingsWindow(answers) {
  const { win, calls } = fakeWindow(ORIGIN);
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => {
    calls.push([cmd, structuredClone(args)]);
    const answer = answers[cmd]?.(args);
    return answer instanceof Error ? Promise.reject(answer.refusal) : Promise.resolve(answer);
  };
  return { win, calls };
}

test("host.muteShortcut and host.callControls are exactly the web's seams", () => {
  const install = loadFactory();
  const { host } = install(fakeWindow(ORIGIN).win, ORIGIN);
  assert.deepEqual(Object.keys(host.muteShortcut).sort(), ["get", "set", "subscribe"]);
  assert.deepEqual(Object.keys(host.callControls).sort(), ["setAlwaysShown", "settings"]);
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

test("a start that asks for a take: the take reaches onTake as a Float32Array, and stop resolves only after it", async () => {
  const install = loadFactory();
  const { win, calls } = voiceWindow();
  win.atob = atob;
  const api = install(win, ORIGIN, "native", true, true);
  const voice = api.host.voice;
  const takes = [];
  voice.onTake((take) => takes.push(take));
  await voice.start({ take: true });
  assert.deepEqual(calls.at(-1), ["voice_start", { take: true }]);
  // The app sends the samples as the base64 of 32-bit little-endian floats (core voice::take).
  const samples = new Float32Array([0, 0.5, -1, 0.25]);
  const base64 = Buffer.from(samples.buffer).toString("base64");
  const turns = [{ turn_id: "c-turn-0", speech_ms: 0, pauses: [{ from_ms: 0, to_ms: 0 }], end_ms: null }];
  let stopped = false;
  const stopping = voice.stop().then(() => { stopped = true; });
  await flush();
  assert.equal(stopped, false, "not before the idle state that answers the stop");
  api.voiceEvent({ type: "take", data: { sampleRate: 16000, samples: base64, started_at: 1000, ended_at: 1000, turns } });
  api.voiceEvent({ type: "state", data: { listening: "idle", recognising: 0, playback: "idle", microphone: "live" } });
  await stopping;
  assert.equal(takes.length, 1);
  assert.equal(Object.prototype.toString.call(takes[0].samples), "[object Float32Array]");
  assert.deepEqual([...takes[0].samples], [...samples]);
  assert.deepEqual({ ...takes[0], samples: null }, { sampleRate: 16000, samples: null, started_at: 1000, ended_at: 1000, turns });
  // A start with no take: its stop does not wait.
  await voice.start();
  assert.deepEqual(calls.at(-1), ["voice_start", { take: false }]);
  await voice.stop();
});
