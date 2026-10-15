// A candidate web PR build is assembled beside this repository for integration checks. Its commit never goes in
// web.pin.json: that file only names a published release and its digest.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { readPin } from "../scripts/web-release.mjs";
import { installSite } from "../scripts/web-site.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const candidate = process.env.SIDEVOICE_CANDIDATE_WEB_SITE;
// Scratch integration head for this draft PR; web.pin.json remains the published archive.
const candidateSha = "2f4e853a5c6e57e64cb36ff1220857d01cb33d10";

test("the lasting web pin remains a release tag with its archive digest", () => {
  const pin = readPin(JSON.parse(readFileSync(path.join(root, "web.pin.json"), "utf8")));
  assert.equal(pin.repository, "sidevoice/sidevoice-web");
  assert.match(pin.sha256, /^[0-9a-f]{64}$/);
});

test("candidate T web site assembles at the desktop paths with turn, playback and hearing contracts", {
  skip: !candidate,
}, () => {
  assert.match(candidateSha, /^[0-9a-f]{40}$/);
  assert.equal(JSON.parse(readFileSync(path.join(candidate, "voice/build-id.json"), "utf8")).build_id,
    `turns-web-${candidateSha}`);
  const scratch = mkdtempSync(path.join(tmpdir(), "sidevoice-turns-bundle-"));
  try {
    assert.ok(installSite(candidate, scratch).includes("voice"));
    assert.ok(existsSync(path.join(scratch, "voice/index.html")));
    assert.ok(existsSync(path.join(scratch, "voice/call-controls.html")));
    assert.ok(existsSync(path.join(scratch, "voice/target.js")));
    const assets = path.join(scratch, "voice/assets");
    const javascript = readdirSync(assets).filter((name) => name.endsWith(".js"))
      .map((name) => readFileSync(path.join(assets, name), "utf8"));
    assert.ok(javascript.some((source) => source.includes("agent-interrupted")), "T voice interruption event");
    assert.ok(javascript.some((source) => source.includes("voice-playback")), "T room playback report");
    assert.ok(javascript.some((source) => source.includes("reply-hearing")), "T semantic hearing report");
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});
