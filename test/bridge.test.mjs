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
    "cancel", "capabilities", "catalogs", "hasCredential", "install", "installed", "memory", "setCredential",
  ]);
  assert.ok(Object.isFrozen(engine));

  assert.equal(JSON.stringify(await engine.capabilities()), JSON.stringify(capabilities));
  assert.equal(JSON.stringify(await engine.installed()), JSON.stringify([{ model: "whisper-tiny", engine: "sherpa-onnx" }]));
  assert.equal(JSON.stringify(await engine.memory()), '{"total_mb":16384,"available_mb":9000}');

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

test("host.engine lists the catalogues and keeps provider keys, refusing with a code", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const catalogs = [
    { id: "local", name: null, status: { stale: false }, models: [{ id: "whisper-tiny", capabilities: ["stt"] }] },
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
  assert.equal(JSON.stringify(await engine.catalogs()), JSON.stringify(catalogs));
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
    "cancelInput", "mute", "onError", "onLevel", "onState", "onTurn", "say", "setSettings", "start", "stop",
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
  await voice.setSettings(settings);
  await voice.start();
  voice.say("Hola.", { language: "es" });
  voice.say("Sin idioma.");
  await voice.mute(1);
  await voice.cancelInput();
  await voice.stop();
  assert.deepEqual(calls.filter(([cmd]) => cmd.startsWith("voice_")), [
    ["voice_set_settings", { settings }],
    ["voice_start", undefined],
    ["voice_say", { key: "say-1", text: "Hola.", language: "es" }],
    ["voice_say", { key: "say-2", text: "Sin idioma.", language: null }],
    ["voice_mute", { muted: true }],
    ["voice_cancel_input", undefined],
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

test("the call's events reach host.voice's listeners by their type, until they stop", () => {
  const install = loadFactory();
  const api = install(voiceWindow().win, ORIGIN, "native", true, true);
  const seen = [];
  const listen = (name) => api.host.voice[name]((data) => seen.push([name, JSON.stringify(data)]));
  const stops = ["onTurn", "onState", "onLevel", "onError"].map(listen);
  api.host.voice.onLevel(() => { throw new Error("a listener's own"); });
  const turn = { phase: "started", turn_id: "c-turn-0", started_at: 1 };
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

test("a refused start rejects with the app's keyed refusal", async () => {
  const install = loadFactory();
  const refusal = { key: "voice_failed", code: "microphone-denied", message: "The call could not start: microphone-denied." };
  const voice = install(voiceWindow({ voice_start: Object.assign(new Error(), { refusal }) }).win, ORIGIN, null, true, true)
    .host.voice;
  await assert.rejects(voice.start(), (error) => error === refusal);
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
  const engine = install(win, ORIGIN).host.engine;
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
  const engine = install(win, ORIGIN).host.engine;
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
  const refusal = { key: "model_needs_memory", model: "whisper-large-v3", needed_mb: 4000, memory_mb: 2000,
    message: "whisper-large-v3 needs 4000 MB of memory; this machine has 2000 MB." };
  win.__TAURI_INTERNALS__.invoke = () => Promise.reject(refusal);
  const engine = install(win, ORIGIN).host.engine;
  await assert.rejects(engine.install("whisper-large-v3", "sherpa-onnx"), (error) => {
    assert.equal(error.key, "model_needs_memory");
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
    "pairRoom", "pairing", "pairingCode", "reconnect", "restart", "revealLog", "serviceInstall", "serviceUninstall",
    "start", "state", "stop", "subscribe",
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
