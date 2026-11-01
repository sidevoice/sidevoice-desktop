// Exercise the pinned page's Media Session implementation against the real native voice bridge.
// Browser/device delivery is deliberately outside this test; no copy of the page's media rules lives here.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import vm from "node:vm";
import { composed } from "./bridge-source.mjs";

const assets = new URL("../ui/voice/assets/", import.meta.url);
const files = readdirSync(assets).filter(name => name.endsWith(".js"));
const chunk = needle => readFileSync(new URL(files.find(name => readFileSync(new URL(name, assets), "utf8").includes(needle)), assets), "utf8");
const controller = chunk("presence.remote");
const provider = chunk("navigator.mediaSession)return null");
const bridge = composed;
const flush = () => new Promise(resolve => setImmediate(resolve));

// The release is minified. Identify the media code by its public seams, preserving its bodies verbatim. Fail when a new
// pin changes this layout, rather than silently skip.
//
// The browser's Media Session and Now Playing element (host/browser-presence.ts): the table of actions, the faint loop and
// `browserRemote()`, side by side, up to the end of that function.
const ofFunction = (source, at) => {
  let depth = 0;
  for (let i = source.indexOf("{", at); i < source.length; i++) {
    if (source[i] === "{") depth++;
    else if (source[i] === "}" && --depth === 0) return i + 1;
  }
  throw new Error("an unclosed function in the pinned page");
};
const actionsAt = provider.match(/const ([\w$]+)=\{play:\(\)=>\(\{type:"set-muted",muted:!1\}\)/);
const remoteAt = provider.match(/function ([\w$]+)\(\)\{if\(typeof navigator>"u"\|\|!navigator\.mediaSession\)return null;/);
assert.ok(actionsAt && remoteAt, "the pinned page keeps its Media Session in one place");
const remoteSource = provider.slice(actionsAt.index, ofFunction(provider, remoteAt.index));
assert.ok(remoteSource.includes("setActionHandler"), "the Media Session's handlers are in that code");
// The page's host gives that Media Session only to an app that does not own the media (host/index.ts, `presence.remote`).
assert.match(provider, /get remote\(\)\{return [\w$]+\.presence\.media==="page"\?[\w$]+\(\):null\}/, "the page withholds its Media Session from an app that owns the media");

// The call's side (room-session-controller.js): priming, showing, applying a command and starting or ending the lock screen.
const prime = controller.match(/function ([\w$]+)\(\)\{const ([\w$]+)=([\w$]+)\(\)\.presence\.remote;\2&&\(/);
assert.ok(prime, "the pinned page primes the host's remote controls");
const host = prime[3];
const primeAt = controller.indexOf(prime[0]);
const start = controller.lastIndexOf("let ", primeAt);
const live = controller.slice(primeAt).match(/function ([\w$]+)\(\)\{return!!([\w$]+)&&([\w$]+)\.micEnabled\}/);
assert.ok(live, "the pinned page exposes its active-call microphone state");
const apply = controller.slice(primeAt).match(/function ([\w$]+)\(([\w$]+)\)\{[\w$]+=\2;const [\w$]+=[\w$]+\(\)\.presence\.remote;/)[1];
const applyAt = controller.indexOf(`function ${apply}(`, primeAt);
const end = controller.indexOf("function ", applyAt + 9);
const region = controller.slice(start, end);
const setter = region.match(/"set-muted"\?([\w$]+)\(![\w$]+\.muted\)/)[1];
const literal = name => name.replaceAll("$", "\\$"); // a minified name may hold a dollar sign
const setterSource = controller.match(new RegExp(`function ${literal(setter)}\\(([\\w$]+)\\)\\{[^{}]+\\}`))[0];
const fanout = setterSource.match(/\.micEnabled=[\w$]+,([\w$]+)\(\)/)[1];
const fanoutSource = controller.match(new RegExp(`function ${literal(fanout)}\\(\\)\\{for\\(const ([\\w$]+) of ([\\w$]+)\\.values\\(\\)\\)[^{}]+\\}`));
assert.ok(fanoutSource, "the page mutes each active voice call");
const setterCalls = [...setterSource.matchAll(/(?:\{|,|&&)([\w$]+)\(\)/g)].map(match => match[1]).filter(name => name !== fanout);
const title = region.match(/([\w$]+)\(([\w$]+)\(\)\)\|\|"Llamada"/);
assert.ok(title, "the pinned page names the lock screen's tile");

/** A page in an app (`owned`: the app says it owns the system's media, as macOS does) at `origin`. */
function page(owned, origin) {
  const calls = [], handlers = new Map(), audio = [];
  const window = {
    location: { protocol: new URL(origin).protocol, host: new URL(origin).host },
    performance: { timeOrigin: 1 },
    // The commands the apps share are a Tauri plugin's, invoked under its name; the test names them as the bridge does. The
    // app numbers the page that opens its channel, and the voice commands carry the number, which the test leaves out.
    __TAURI_INTERNALS__: {
      transformCallback: () => 1,
      invoke(command, args) {
        const name = command.replace(/^plugin:sidevoice-app-tauri\|/, "");
        if (name === "voice_events") return Promise.resolve(1);
        const { page, ...others } = args ?? {};
        calls.push([name, args && "page" in args ? others : args]);
        return Promise.resolve();
      },
    },
    setInterval() {}, clearInterval() {}, setTimeout,
  };
  const module = { exports: {} };
  vm.runInNewContext(bridge, { module, Promise, JSON, Date });
  const desktop = module.exports(window, origin, owned, false, true);
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
  // The page's host: its Media Session where the app does not own the media, none where it does.
  vm.runInContext(`${remoteSource}\n${fanoutSource[0]}\n${setterSource}\n${region}
    globalThis.${host} = () => ({ presence: { media: ${owned ? '"app"' : '"page"'}, remote: ${owned ? "null" : `${remoteAt[1]}()`} } });
    globalThis.media = { prime: ${prime[1]}, apply: ${apply} };`, context);
  return { desktop, call, calls, handlers, audio, media: context.media };
}

for (const [os, origin] of [["Windows", "http://tauri.localhost"], ["Linux", "tauri://localhost"]]) {
  test(`${os}: page Media Session controls a native call when native media controls are unavailable`, async () => {
    const p = page(false, origin);
    await p.call.ready;
    assert.ok(p.desktop.host.voice, "native voice is enabled independently of media controls");
    assert.equal(p.desktop.host.presence, undefined, "the app owns no media here, and does not say it does");
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
  const p = page(true, "tauri://localhost");
  await p.call.ready;
  assert.deepEqual(JSON.parse(JSON.stringify(p.desktop.host.presence)), { media: "app" });
  p.media.prime();
  p.media.apply(true);
  assert.equal(p.audio.length, 0);
  assert.equal(p.handlers.size, 0);
});
