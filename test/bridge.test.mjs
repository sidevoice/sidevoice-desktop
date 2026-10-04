// Unit tests of bridge/desktop-bridge.js against a fake window: no browser, no Tauri.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
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
const ORIGIN = "https://voice.example.com";

test("does nothing on any other origin (auth pages, other sites)", () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow("https://accounts.google.com");
  assert.equal(install(win, ORIGIN), null);
  assert.equal(win.__sidevoiceDesktop, undefined);
  assert.equal(calls.length, 0);
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
    version: 3, ready: true, joined: false, busy: false, micEnabled: true, micDisabled: false, title: "Claude",
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
  const audioDevices = { inputs: [{ id: "default", label: "x" }], outputs: [], inputId: "default", outputId: "default", available: true, outputAvailable: false, busy: false, extra: 1 };
  const store = fakeStore({ call: { joined: false }, mic: { enabled: true }, title: "a browsed transcript", participants, audioDevices,
    callCard: { agent: "idle", youTalking: false, canSkip: false, title: "mini" } });
  win.sidevoiceUI = { store };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {} };
  const api = install(win, ORIGIN);
  await flush();

  let s = api.snapshot();
  assert.equal(s.since, null, "no clock outside a call");
  const plain = (value) => JSON.parse(JSON.stringify(value)); // objects made in the bridge's own realm
  assert.deepEqual(plain(s.participants[0]), { threadId: "a", title: "mini", selected: true, reach: "listening", machine: "daimon", harness: "claude", working: false, subtitle: "" });
  assert.deepEqual(plain(s.devices), { inputs: [{ id: "default", label: "x" }], outputs: [], inputId: "default", outputId: "default", available: true, outputAvailable: false, busy: false });

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
    selectAudioDevice: async (kind, id) => did.push("device " + kind + " " + id),
  };
  const api = install(win, ORIGIN);
  assert.equal(api.run({ command: "skip-reply" }), false, "nothing playing");
  store.set({ callCard: { canSkip: true } });
  assert.equal(api.run({ command: "skip-reply" }), true);
  assert.equal(api.run({ command: "select-participant", threadId: "b" }), true);
  assert.equal(api.run({ command: "select-participant", threadId: "zzz" }), false, "only a conversation the room lists");
  assert.equal(api.run({ command: "select-participant", threadId: "c" }), false, "and one the call can go to");
  assert.equal(api.run({ command: "select-audio-device", kind: "input", id: "mbp" }), true);
  assert.equal(api.run({ command: "select-audio-device", kind: "camera", id: "x" }), false);
  await flush();
  assert.deepEqual(did, ["skip", "select b", "device input mbp"]);
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

test("the native engine is exactly the contract, keyed by catalogue model id + engine, audio as raw bytes", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const capabilities = { runs: "native", os: "macos", arch: "aarch64", has: ["cpu", "coreml"], memory_mb: 16384 };
  const answers = {
    engine_capabilities: capabilities,
    engine_installed: [{ model: "whisper-tiny", engine: "sherpa-onnx" }],
    engine_synthesize: (() => {
      const buffer = new ArrayBuffer(4 + 8);
      new DataView(buffer).setUint32(0, 24000, true);
      new Float32Array(buffer, 4, 2).set([0.5, -0.25]);
      return buffer;
    })(),
    engine_transcribe: "hola",
    engine_install: () => new Promise((resolve) => setTimeout(() => resolve(null), 20)), // long enough to poll
    engine_progress: { job: "?", model: "whisper-small", engine: "sherpa-onnx", done: 5, total: 10, bytes_per_s: null },
  };
  win.__TAURI_INTERNALS__.invoke = (cmd, args, options) => {
    calls.push([cmd, args, options]);
    return Promise.resolve(typeof answers[cmd] === "function" ? answers[cmd]() : answers[cmd]);
  };
  const engine = install(win, ORIGIN).host.nativeEngine;
  assert.equal(install(win, ORIGIN).host.app, "sidevoice-desktop");
  assert.deepEqual(Object.keys(engine).sort(),
    ["cancel", "capabilities", "install", "installed", "load", "loaded", "memory", "synthesize", "transcribe", "unload"]);
  assert.ok(Object.isFrozen(engine));

  assert.equal(JSON.stringify(await engine.capabilities()), JSON.stringify(capabilities));
  assert.equal(JSON.stringify(await engine.installed()), JSON.stringify([{ model: "whisper-tiny", engine: "sherpa-onnx" }]));

  const samples = new Float32Array([0.1, 0.2, 0.3]);
  assert.equal(await engine.transcribe("whisper-small", "sherpa-onnx", samples, 16000, "es"), "hola");
  const [, body, options] = calls.find(([cmd]) => cmd === "engine_transcribe");
  assert.ok(body.constructor.name === "Uint8Array" && body.byteLength === 12, "raw f32 bytes");
  // (objects built inside the vm context: compare their JSON, not their prototypes)
  assert.equal(JSON.stringify(options.headers), JSON.stringify({
    "x-model": "whisper-small", "x-engine": "sherpa-onnx", "x-accelerator": "", "x-language": "es", "x-sample-rate": "16000",
  }));
  await engine.transcribe("whisper-small", "sherpa-onnx", samples, 16000, "", "coreml");
  assert.equal(calls.filter(([cmd]) => cmd === "engine_transcribe")[1][2].headers["x-accelerator"], "coreml");

  const audio = await engine.synthesize("kokoro-82m-v1.0", "sherpa-onnx", "ef_dora", 1, "hola");
  assert.equal(audio.sampleRate, 24000);
  assert.deepEqual(Array.from(audio.samples), [0.5, -0.25]);
  assert.equal(JSON.stringify(calls.find(([cmd]) => cmd === "engine_synthesize")[1]), JSON.stringify({
    model: "kokoro-82m-v1.0", engine: "sherpa-onnx", accelerator: null, voice: "ef_dora", speed: 1, text: "hola",
  }));
  await engine.synthesize("kokoro-82m-v1.0", "sherpa-onnx", "ef_dora", 1, "hola", "cpu");
  assert.equal(calls.filter(([cmd]) => cmd === "engine_synthesize")[1][1].accelerator, "cpu");

  answers.engine_synthesize = Array.from(new Uint8Array(answers.engine_synthesize));
  assert.equal((await engine.synthesize("kokoro-82m-v1.0", "sherpa-onnx", "ef_dora", 1, "x")).sampleRate, 24000, "postMessage fallback");

  const seen = [];
  const installing = engine.install("whisper-small", "sherpa-onnx", (progress) => seen.push(progress));
  assert.match(installing.job, /^install-/, "the call's own job, there from the start");
  await installing;
  const installArgs = calls.find(([cmd]) => cmd === "engine_install")[1];
  assert.deepEqual([installArgs.model, installArgs.engine, installArgs.job], ["whisper-small", "sherpa-onnx", installing.job]);
  assert.equal(calls.find(([cmd]) => cmd === "engine_progress")[1].job, installArgs.job, "polled by that job");
  assert.equal(JSON.stringify(seen[0]), JSON.stringify(answers.engine_progress), "one object per report, as the app sent it");
  assert.match(engine.install("whisper-tiny", "sherpa-onnx").job, /^install-/, "without onProgress too");
});

test("load, unload, loaded and memory: the build in memory, by catalogue model id + engine", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const resident = [{ model: "whisper-small", engine: "sherpa-onnx", accelerator: "coreml", since: 1, last_used: 2 }];
  const answers = {
    engine_load: { load_ms: 840 },
    engine_unload: null,
    engine_loaded: resident,
    engine_memory: { total_mb: 16384, available_mb: 9000 },
  };
  win.__TAURI_INTERNALS__.invoke = (cmd, args) => (calls.push([cmd, structuredClone(args)]), Promise.resolve(answers[cmd]));
  const engine = install(win, ORIGIN).host.nativeEngine;

  assert.equal(JSON.stringify(await engine.load("whisper-small", "sherpa-onnx", "coreml")), '{"load_ms":840}');
  await engine.load("kokoro-82m-v1.0", "sherpa-onnx");
  assert.equal(await engine.unload("whisper-small", "sherpa-onnx"), null);
  await engine.unload("whisper-small", "sherpa-onnx", "coreml");
  assert.equal(JSON.stringify(await engine.loaded()), JSON.stringify(resident));
  assert.equal(JSON.stringify(await engine.memory()), '{"total_mb":16384,"available_mb":9000}');
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("engine_")), [
    ["engine_load", { model: "whisper-small", engine: "sherpa-onnx", accelerator: "coreml" }],
    ["engine_load", { model: "kokoro-82m-v1.0", engine: "sherpa-onnx", accelerator: null }],
    ["engine_unload", { model: "whisper-small", engine: "sherpa-onnx", accelerator: null }],
    ["engine_unload", { model: "whisper-small", engine: "sherpa-onnx", accelerator: "coreml" }],
    ["engine_loaded", undefined],
    ["engine_memory", undefined],
  ]);

  const refusal = { key: "runtime_failed", engine: "sherpa-onnx", message: "sherpa-onnx failed: …" };
  win.__TAURI_INTERNALS__.invoke = () => Promise.reject(refusal);
  await assert.rejects(engine.load("whisper-small", "sherpa-onnx"), (error) => error === refusal, "refusals pass through as sent");
});

test("cancel: by the job install carries from the start; the install rejects as the app refuses it", async () => {
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
  const engine = install(win, ORIGIN).host.nativeEngine;
  const seen = [];
  const installing = engine.install("whisper-small", "sherpa-onnx", (p) => seen.push(p));
  await flush();
  assert.equal(await engine.cancel(installing.job), true);
  await assert.rejects(installing, (error) => error.key === "install_cancelled" && error.message === "The download was cancelled.");
  assert.equal(await engine.cancel(installing.job), false, "an ended job: nothing to cancel, no throw");
  assert.deepEqual(calls.filter(([cmd]) => cmd === "engine_cancel").map(([, args]) => args), [{ job: installing.job }, { job: installing.job }]);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(seen, [], "no progress for it once it ended");
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
  const engine = install(win, ORIGIN).host.nativeEngine;
  const whisper = [];
  const kokoro = [];
  const a = engine.install("whisper-small", "sherpa-onnx", (p) => whisper.push([p.done, p.total, p.job]));
  const b = engine.install("kokoro-82m-v1.0", "sherpa-onnx", (p) => kokoro.push([p.done, p.total, p.job]));
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
    assert.ok(kokoro.every(([, t, job]) => t === 132 && job === b.job) && kokoro.length > 0, "B sees only its own");
    assert.ok(whisper.every(([, , job]) => job === a.job));
    finish.get(jobs[1])();
    await b;
  } finally {
    for (const done of finish.values()) done(); // never leave a poller running
  }
});

test("a refusal reaches the page as the app sent it: a key, its parameters and an English message", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  const refusal = { key: "not_installed", model: "whisper-small", engine: "sherpa-onnx", message: "whisper-small on sherpa-onnx is not downloaded yet." };
  win.__TAURI_INTERNALS__.invoke = () => Promise.reject(refusal);
  const engine = install(win, ORIGIN).host.nativeEngine;
  await assert.rejects(engine.transcribe("whisper-small", "sherpa-onnx", new Float32Array(1), 16000, "es"), (error) => {
    assert.equal(error.key, "not_installed");
    assert.equal(error.model, "whisper-small");
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
});

test("the placeholders the app replaces are present exactly once", () => {
  assert.equal(source.split("__SIDEVOICE_ROOM_ORIGIN__").length, 2);
  assert.equal(source.split("__SIDEVOICE_MEDIA_KEYS__").length, 2);
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
    assert.equal(api.host.version, 3);
    assert.equal(api.host.app, "sidevoice-desktop");
    assert.ok(api.host.nativeEngine, "the rest of the host is untouched");
    assert.equal("localHost" in api.host, false, String(offered));
  }
  const { win } = fakeWindow(ORIGIN);
  const api = install(win, ORIGIN, "native", true);
  assert.deepEqual(Object.keys(api.host.localHost).sort(), [
    "agents", "cancel", "install", "pairRoom", "pairing", "pairingCode", "reconnect", "restart", "revealLog",
    "serviceInstall", "serviceUninstall", "start", "state", "stop", "subscribe", "update", "version",
  ]);
  assert.ok(Object.isFrozen(api.host.localHost));
});

test("localHost install progress and cancellation stay scoped to their job", async () => {
  const install = loadFactory();
  const calls = [];
  let resolveInstall;
  const { win, tick, cleared } = localHostWindow({
    local_host_install: () => new Promise((resolve) => { resolveInstall = resolve; }),
    local_host_install_progress: ({ job, afterSequence }) => [
      { job, sequence: afterSequence + 1, step: "download", done: 4, total: 12, cancellable: true },
      { job, sequence: afterSequence + 2, step: "verify", done: null, total: null, cancellable: false },
      { job, sequence: afterSequence + 3, step: "stage", done: null, total: null, cancellable: false },
      { job, sequence: afterSequence + 4, step: "wait-lock", done: null, total: null, cancellable: false },
      { job, sequence: afterSequence + 5, step: "wait-calls", done: null, total: null, cancellable: false },
    ],
    local_host_cancel: ({ job }) => (calls.push(job), true),
    local_host_update: { state: "running", reachable: true },
    local_host_version: { bridge: 3, update: "available", capabilities: { agents: false } },
  });
  const local = install(win, ORIGIN, null, true).host.localHost;
  const progress = [];
  const pending = local.install((event) => progress.push(event));
  assert.match(pending.job, /^local-install-/);
  tick();
  await flush();
  assert.deepEqual(JSON.parse(JSON.stringify(progress)), [
    { step: "download", done: 4, total: 12, cancellable: true },
    { step: "verify", done: null, total: null, cancellable: false },
    { step: "staging", done: null, total: null, cancellable: false },
    { step: "staging", done: null, total: null, cancellable: false },
    { step: "staging", done: null, total: null, cancellable: false },
  ]);
  assert.equal(await local.cancel(pending.job), true);
  assert.deepEqual(await local.version(), { bridge: 3, update: "available", capabilities: { agents: false } });
  assert.deepEqual(await local.update(), { state: "running", reachable: true });
  resolveInstall({ state: "running", reachable: true });
  assert.deepEqual(await pending, { state: "running", reachable: true });
  assert.equal(cleared.length, 1);
  assert.deepEqual(calls, [pending.job]);
});

test("localHost flushes final install progress when the install settles before the first poll", async () => {
  const install = loadFactory();
  const calls = [];
  const { win } = localHostWindow({
    local_host_install: { state: "running", reachable: true },
    local_host_install_progress: ({ job, afterSequence }) => {
      calls.push({ job, afterSequence });
      return afterSequence === 0
        ? [{ job, sequence: 1, step: "pairing", done: null, total: null, cancellable: false }]
        : [];
    },
  });
  const local = install(win, ORIGIN, null, true).host.localHost;
  const progress = [];
  assert.deepEqual(await local.install((event) => progress.push(event)), { state: "running", reachable: true });
  assert.deepEqual(JSON.parse(JSON.stringify(progress)), [
    { step: "pairing", done: null, total: null, cancellable: false },
  ]);
  assert.equal(calls.length, 1, "the settlement path performs a final progress poll");
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

test("localHost passes the safe connector authenticity check through to web i18n", async () => {
  const install = loadFactory();
  const refusal = { key: "install.authenticity", message: "The connector could not verify the core's authenticity.",
    params: { check: "repository-id" } };
  const failed = Object.assign(new Error("refused"), { refusal });
  const { win } = localHostWindow({ local_host_install: failed });
  await assert.rejects(install(win, ORIGIN, null, true).host.localHost.install(), (error) => {
    assert.deepEqual(error, refusal);
    return true;
  });
});

test("agents capability is explicit and unavailable until connector R2", async () => {
  const install = loadFactory();
  const refused = Object.assign(new Error("unavailable"), { refusal: { key: "agents.unavailable", message: "m" } });
  const { win, calls } = localHostWindow({ local_host_agents: refused });
  await assert.rejects(install(win, ORIGIN, null, true).host.localHost.agents(), { key: "agents.unavailable", message: "m" });
  assert.deepEqual(calls.filter(([command]) => command.startsWith("local_host")), [["local_host_agents", undefined]]);
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

test("nothing of localHost runs until the page asks", async () => {
  const install = loadFactory();
  const { win, calls, tick } = localHostWindow({ local_host_state: { state: "running" } });
  install(win, ORIGIN, null, true);
  tick();
  await flush();
  assert.equal(calls.filter(([cmd]) => cmd.startsWith("local_host")).length, 0);
});
