// The thinking sounds the iOS app plays (`host.cue`, docs/BRIDGE.md -> "The cues") against the web interface the app bundles
// (web.pin.json, ui/): the app finds them where mobile/tauri/plugin/src/cue.rs looks, in the assets Tauri serves from ui/, so
// a manifest or a file the pinned web moved or lost would leave the phone silent, and it fails here, naming it.
// The sounds are in the web from the release that adds them: a pin from before it has none, and the app then offers no player
// (`cue_available`, so the page plays its own). Bumping web.pin.json past it turns this on by itself.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";

const cue = readFileSync(new URL("../mobile/tauri/plugin/src/cue.rs", import.meta.url), "utf8");
const constant = (name) => {
  const found = new RegExp(`const ${name}: &str = "([^"]+)";`).exec(cue);
  assert.ok(found, `cue.rs has no ${name}`);
  return found[1];
};
const MANIFEST = constant("SOUNDS_MANIFEST");
const FOLDER = constant("SOUNDS_FOLDER");
const ui = new URL("../ui/", import.meta.url);
const bundled = existsSync(new URL(MANIFEST, ui));

test("the app looks for the sounds beside their manifest, in the web it serves under voice/", () => {
  assert.equal(MANIFEST, `${FOLDER}sounds.json`);
  assert.ok(FOLDER.startsWith("voice/"), "the web is served from ui/voice/ (scripts/web-site.mjs)");
  assert.ok(existsSync(new URL("voice/index.html", ui)), "the pinned web was not found in ui/voice: npm run web");
});

test("every sound of the pinned web is where the app reads it, an MP4 file (its ftyp box)", { skip: !bundled && "the pinned web has no thinking sounds yet" }, () => {
  const manifest = JSON.parse(readFileSync(new URL(MANIFEST, ui), "utf8"));
  assert.ok(manifest.sounds.length > 0);
  assert.ok(manifest.sounds.some((sound) => sound.id === manifest.default), `the default ${manifest.default} is one of the sounds`);
  for (const sound of manifest.sounds) {
    const [min, max] = sound.interval;
    assert.ok(min >= 0 && max >= min, `${sound.id}: an interval of [${sound.interval}]`);
    const file = new URL(FOLDER + sound.files.m4a, ui);
    assert.ok(existsSync(file), `${sound.id}: ${FOLDER}${sound.files.m4a} is not in the pinned web`);
    assert.equal(readFileSync(file).subarray(4, 8).toString("latin1"), "ftyp", `${sound.id}: not an MP4`);
  }
});
