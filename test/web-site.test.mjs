// scripts/web-site.mjs: a pin's site replaces the previous one in ui/, directory for directory, and leaves the app's
// own files alone.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { installSite } from "../scripts/web-site.mjs";

function site(root, files) {
  for (const [file, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    writeFileSync(path.join(root, file), text);
  }
  return root;
}

test("a directory the previous pin built and the new one does not is removed", () => {
  const work = mkdtempSync(path.join(tmpdir(), "web-site-"));
  const ui = site(path.join(work, "ui"), { "index.html": "settings", "brand/logo.svg": "logo" });
  const old = site(path.join(work, "old"), {
    "voice/index.html": "old room",
    "voice-browser/native-worker.js": "old worker",
  });
  const next = site(path.join(work, "next"), { "voice/index.html": "new room" });

  assert.deepEqual(installSite(old, ui).sort(), ["voice", "voice-browser"]);
  assert.ok(existsSync(path.join(ui, "voice-browser/native-worker.js")));

  assert.deepEqual(installSite(next, ui), ["voice"]);
  assert.equal(existsSync(path.join(ui, "voice-browser")), false, "the old pin's directory is gone");
  assert.equal(readFileSync(path.join(ui, "voice/index.html"), "utf8"), "new room");
  assert.equal(readFileSync(path.join(ui, "index.html"), "utf8"), "settings", "the app's own files stay");
  assert.equal(readFileSync(path.join(ui, "brand/logo.svg"), "utf8"), "logo");

  // The same pin again (a warm cache): the same result.
  assert.deepEqual(installSite(next, ui), ["voice"]);
  assert.equal(readFileSync(path.join(ui, "voice/index.html"), "utf8"), "new room");
});
