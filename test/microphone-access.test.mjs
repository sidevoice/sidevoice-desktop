// The current Windows/Linux capture backend is the webview. Test its observations without replacing that backend
// with a second native capture implementation. OS privacy/device integration remains a separate runtime check.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const source = readFileSync(new URL("../bridge/desktop-bridge.js", import.meta.url), "utf8");
function setup({ permission = "prompt", devices = [{ kind: "audioinput" }], failure, live = true, emptyAudio = false, settings = true } = {}) {
  const module = { exports: {} };
  vm.runInNewContext(source, { module, Promise, JSON, Date });
  const calls = [];
  let stopped = 0;
  let opened = 0;
  const track = { readyState: live ? "live" : "ended", stop: () => stopped++ };
  const media = {
    enumerateDevices: async () => { if (devices instanceof Error) throw devices; return devices; },
    getUserMedia: async (constraints) => {
      opened++;
      assert.equal(constraints.audio, true);
      if (failure) throw failure;
      return { getAudioTracks: () => emptyAudio ? [] : [track], getTracks: () => [track] };
    },
  };
  const win = {
    setInterval: () => 1,
    clearInterval() {},
    location: { protocol: "https:", host: "voice.example.com" },
    navigator: { mediaDevices: media, permissions: { query: async () => {
      if (permission instanceof Error) throw permission;
      return { state: permission };
    } } },
    __TAURI_INTERNALS__: { invoke: async (cmd, args) => {
      if (cmd.startsWith("microphone")) calls.push([cmd, structuredClone(args)]);
      return "unavailable"; // Rust owns normalization and Windows' privacy override.
    } },
  };
  const host = module.exports(win, "https://voice.example.com", null, false, false, false, undefined, undefined, undefined, settings).host;
  return { host, calls, stopped: () => stopped, opened: () => opened, setPermission: (value) => { permission = value; } };
}

test("the richer access member preserves the page permission fallback and Windows settings command", async () => {
  const { host, calls } = setup();
  assert.deepEqual(Object.keys(host.microphone).sort(), ["access", "openSettings"]);
  assert.equal(host.microphone.state, undefined);
  assert.equal(host.microphone.request, undefined);
  assert.ok(Object.isFrozen(host.microphone.access));
  await host.microphone.openSettings();
  assert.deepEqual(calls, [["microphone_open_settings", undefined]]);
  assert.deepEqual(Object.keys(setup({ settings: false }).host.microphone), ["access"], "Linux has no settings URI");
});

test("inspection reads permission changes and device presence afresh, without opening capture", async () => {
  const app = setup({ devices: [] });
  await app.host.microphone.access.state();
  app.setPermission("denied");
  await app.host.microphone.access.state();
  app.setPermission("granted");
  await app.host.microphone.access.state();
  assert.deepEqual(app.calls.map(([, args]) => args.backend), [
    { permission: "not-determined", hasDevice: false },
    { permission: "denied", hasDevice: false },
    { permission: "granted", hasDevice: false },
  ]);
  assert.equal(app.opened(), 0);
});

test("an unreadable permission or device list remains unknown", async () => {
  const app = setup({ permission: new Error("unsupported"), devices: new Error("backend unavailable") });
  assert.equal(await app.host.microphone.access.state(), "unavailable");
  assert.deepEqual(app.calls[0], ["microphone", {
    action: "access-state", backend: { permission: "unavailable", hasDevice: null },
  }]);
});

test("request releases capture and reports a live audio track as granted", async () => {
  const app = setup();
  await app.host.microphone.access.request();
  assert.equal(app.stopped(), 1);
  assert.equal(app.opened(), 1);
  assert.deepEqual(app.calls[0], ["microphone", {
    action: "access-request", backend: { captured: true, hasDevice: true },
  }]);
});

test("ended or absent audio tracks do not fabricate evidence of missing hardware", async () => {
  // The default fixture still enumerates an audio input: a revoked/failed track is not device absence.
  for (const options of [{ live: false }, { emptyAudio: true }]) {
    const app = setup(options);
    await app.host.microphone.access.request();
    assert.equal(app.stopped(), 1, "even unusable capture releases every track");
    assert.equal(app.opened(), 1);
    assert.deepEqual(app.calls[0], ["microphone", {
      action: "access-request", backend: { permission: "unavailable" },
    }]);
  }
});

test("capture errors retain backend names for Rust's denial/no-device/unavailable mapping", async () => {
  for (const name of ["NotAllowedError", "NotFoundError", "NotReadableError", "AbortError", "OverconstrainedError"]) {
    const app = setup({ failure: { name } });
    await app.host.microphone.access.request();
    assert.deepEqual(app.calls[0], ["microphone", { action: "access-request", backend: { error: name } }]);
    assert.equal(app.stopped(), 0);
  }
});
