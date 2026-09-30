// Unit tests of bridge/desktop-bridge.js against a fake window: no browser, no Tauri.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../bridge/desktop-bridge.js", import.meta.url), "utf8");

function loadFactory() {
  const module = { exports: {} };
  vm.runInNewContext(source, { module, Promise, JSON });
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
  assert.equal(api.run("toggle-mute"), false, "nothing to drive yet");

  const store = fakeStore({ call: { joined: false, busy: false }, mic: { enabled: true, disabled: false }, title: "Claude" });
  win.sidevoiceUI = { store };
  win.sidevoiceActions = { toggleMic() {}, toggleCall: async () => {} };
  tick();
  await flush();
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[1][1].snapshot, {
    version: 1, ready: true, joined: false, busy: false, micEnabled: true, micDisabled: false, title: "Claude",
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

  assert.equal(api.run("hang-up"), false, "no call to hang up");
  assert.equal(calls, 0);
  assert.equal(api.run("toggle-mute"), true);
  assert.equal(mics, 1);
  assert.equal(api.snapshot().micEnabled, false);

  store.set({ call: { joined: true } });
  assert.equal(api.run("hang-up"), true);
  assert.equal(calls, 1);

  store.set({ mic: { enabled: true, disabled: true } });
  assert.equal(api.run("toggle-mute"), false, "disabled in the web UI, disabled here");
  assert.equal(mics, 1);
  assert.equal(api.run("rm -rf"), false);
});

test("installs once per page and survives a missing Tauri runtime", async () => {
  const install = loadFactory();
  const { win } = fakeWindow(ORIGIN);
  delete win.__TAURI_INTERNALS__;
  const first = install(win, ORIGIN);
  assert.equal(install(win, ORIGIN), first);
  await flush();
});

test("the native engine goes through the app's commands, audio as raw bytes", async () => {
  const install = loadFactory();
  const { win, calls } = fakeWindow(ORIGIN);
  const answers = {
    engine_available: { device: { os: "macos" }, offers: [], pageIds: {} },
    engine_synthesize: (() => {
      const buffer = new ArrayBuffer(4 + 8);
      new DataView(buffer).setUint32(0, 24000, true);
      new Float32Array(buffer, 4, 2).set([0.5, -0.25]);
      return buffer;
    })(),
    engine_transcribe: "hola",
    engine_install: null,
    engine_progress: [5, 10],
  };
  win.__TAURI_INTERNALS__.invoke = (cmd, args, options) => {
    calls.push([cmd, args, options]);
    return Promise.resolve(answers[cmd]);
  };
  const engine = install(win, ORIGIN).host.nativeEngine;
  assert.equal(install(win, ORIGIN).host.app, "sidevoice-desktop");

  assert.equal((await engine.available()).device.os, "macos");

  const samples = new Float32Array([0.1, 0.2, 0.3]);
  assert.equal(await engine.transcribe("whisper-tiny", samples, 16000, "es"), "hola");
  const [, body, options] = calls.find(([cmd]) => cmd === "engine_transcribe");
  assert.ok(body.constructor.name === "Uint8Array" && body.byteLength === 12, "raw f32 bytes");
  // (objects built inside the vm context: compare their JSON, not their prototypes)
  assert.equal(JSON.stringify(options.headers), JSON.stringify({ "x-model": "whisper-tiny", "x-language": "es", "x-sample-rate": "16000" }));

  const audio = await engine.synthesize("kokoro-82m-v1.0", "ef_dora", 1, "hola");
  assert.equal(audio.sampleRate, 24000);
  assert.deepEqual(Array.from(audio.samples), [0.5, -0.25]);

  answers.engine_synthesize = Array.from(new Uint8Array(answers.engine_synthesize));
  assert.equal((await engine.synthesize("kokoro-82m-v1.0", "ef_dora", 1, "x")).sampleRate, 24000, "postMessage fallback");

  const seen = [];
  await engine.install("whisper-tiny", undefined, (done, total) => seen.push([done, total]));
  assert.equal(JSON.stringify(calls.find(([cmd]) => cmd === "engine_install")[1]), JSON.stringify({ model: "whisper-tiny", engine: "sherpa-onnx" }));
});

test("works on the app's own pages, whose origin may be opaque", () => {
  const install = loadFactory();
  const { win } = fakeWindow("tauri://localhost");
  assert.ok(install(win, "tauri://localhost"));
  const other = fakeWindow("tauri://localhost");
  assert.equal(install(other.win, ORIGIN), null);
});

test("the placeholder the app replaces is present exactly once", () => {
  assert.equal(source.split("__SIDEVOICE_ROOM_ORIGIN__").length, 2);
});
