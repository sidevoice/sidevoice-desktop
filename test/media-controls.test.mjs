// Exercise the pinned page's Media Session implementation against the real native voice bridge.
// Browser/device delivery is deliberately outside this test; no copy of the page's media rules lives here.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import vm from "node:vm";

const assets = new URL("../ui/voice/assets/", import.meta.url);
const controller = readFileSync(new URL(readdirSync(assets).find(name => /^room-session-controller-.*\.js$/.test(name)), assets), "utf8");
const bridge = readFileSync(new URL("../bridge/desktop-bridge.js", import.meta.url), "utf8");
const flush = () => new Promise(resolve => setImmediate(resolve));

// The release is minified. Identify the media region and its microphone setter by their public seams,
// preserving their bodies verbatim. Fail when a new pin changes this layout, rather than silently skip.
const mediaAt = controller.indexOf("mediaKeys");
const prime = controller.slice(controller.lastIndexOf("function ", mediaAt), mediaAt).match(/^function (\w+)\(/)[1];
const start = controller.lastIndexOf("let ", controller.lastIndexOf("=null;function", mediaAt));
const live = controller.slice(mediaAt).match(/function (\w+)\(\)\{return!!(\w+)&&(\w+)\.micEnabled\}/);
assert.ok(live, "the pinned page exposes its active-call microphone state");
const apply = controller.slice(mediaAt).match(/function (\w+)\((\w+)\)\{\w+=\2;const/)[1];
const applyAt = controller.indexOf(`function ${apply}(`, mediaAt);
const end = controller.indexOf("function ", applyAt + 9);
const region = controller.slice(start, end);
const setter = region.match(/play:\(\)=>(\w+)\(!0\)/)[1];
const setterSource = controller.match(new RegExp(`function ${setter}\\((\\w+)\\)\\{[^{}]+\\}`))[0];
const fanout = setterSource.match(/\.micEnabled=\w+,(\w+)\(\)/)[1];
const fanoutSource = controller.match(new RegExp(`function ${fanout}\\(\\)\\{for\\(const (\\w+) of (\\w+)\\.values\\(\\)\\)[^{}]+\\}`));
assert.ok(fanoutSource, "the page mutes each active voice call");
const setterCalls = [...setterSource.matchAll(/(?:\{|,|&&)(\w+)\(\)/g)].map(match => match[1]).filter(name => name !== fanout);
const title = region.match(/artist:(\w+)\((\w+)\(\)\)/);

function page(mediaKeys, origin) {
  const calls = [], handlers = new Map(), audio = [];
  const window = {
    location: { protocol: new URL(origin).protocol, host: new URL(origin).host },
    // The commands the apps share are a Tauri plugin's, invoked under its name; the test names them as the bridge does.
    __TAURI_INTERNALS__: { invoke(command, args) { calls.push([command.replace(/^plugin:sidevoice-app-tauri\|/, ""), args]); return Promise.resolve(); } },
    setInterval() {}, clearInterval() {}, setTimeout,
  };
  const module = { exports: {} };
  vm.runInNewContext(bridge, { module, Promise, JSON, Date });
  const desktop = module.exports(window, origin, mediaKeys, false, true);
  const call = desktop.host.voice.start({ input: { model: "test" } });
  desktop.voiceEvent({ key: call.id, type: "state", data: { listening: "listening" } });
  const context = vm.createContext({
    window, Blob, DataView, ArrayBuffer, URL,
    navigator: { mediaSession: { setActionHandler: (name, handler) => handlers.set(name, handler) } },
    MediaMetadata: class { constructor(value) { Object.assign(this, value); } },
    Audio: class {
      constructor() { this.paused = true; audio.push(this); }
      setAttribute() {}
      play() { this.paused = false; return Promise.resolve(); }
      pause() { this.paused = true; }
    },
    [live[2]]: call,
    [live[3]]: { micEnabled: true },
    [fanoutSource[2]]: new Map([[call.id, { call }]]),
    [title[1]]: () => "Call", [title[2]]: () => "thread",
    ...Object.fromEntries(setterCalls.map(name => [name, () => {}])),
  });
  vm.runInContext(`${fanoutSource[0]}\n${setterSource}\n${region}\nglobalThis.media = { prime: ${prime}, apply: ${apply} };`, context);
  return { desktop, call, calls, handlers, audio, media: context.media };
}

for (const [os, origin] of [["Windows", "http://tauri.localhost"], ["Linux", "tauri://localhost"]]) {
  test(`${os}: page Media Session controls a native call when native media controls are unavailable`, async () => {
    const p = page(null, origin);
    await p.call.ready;
    assert.ok(p.desktop.host.voice, "native voice is enabled independently of media controls");
    assert.equal(p.desktop.host.mediaKeys, null);
    p.media.prime();
    p.media.apply(true);
    assert.equal(p.audio.length, 1, "the page claims Now Playing during the native call");
    assert.equal(p.audio[0].paused, false);
    p.handlers.get("pause")();
    await flush();
    assert.equal(p.audio[0].paused, true);
    p.handlers.get("pause")(); // A headset click can arrive as pause even while muted.
    p.handlers.get("play")();
    p.handlers.get("togglemicrophone")({ isActivating: false });
    p.handlers.get("togglemicrophone")({ isActivating: true });
    await flush();
    assert.deepEqual(p.calls.filter(([name]) => name === "voice_mute").map(([, args]) => args.muted),
      [true, false, false, true, false]);
    assert.ok(p.calls.filter(([name]) => name === "voice_mute").every(([, args]) => args.key === p.call.id));
    p.media.apply(false);
    assert.ok([...p.handlers.values()].every(handler => handler === null), "hang-up removes the handlers");
    assert.equal(p.audio[0].paused, true);
  });
}

test("macOS: native media ownership prevents duplicate page handlers and Now Playing", async () => {
  const p = page("native", "tauri://localhost");
  await p.call.ready;
  p.media.prime();
  p.media.apply(true);
  assert.equal(p.audio.length, 0);
  assert.equal(p.handlers.size, 0);
});
