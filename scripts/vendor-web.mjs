#!/usr/bin/env node
// Copies the Sidevoice web interface, already built in a rubasace/sidevoice checkout, into ui/ at the
// paths the room serves it from (/voice/, /voice-browser/), so the app can show it for a node that serves
// no page. Records where it came from in ui/voice/web-source.json.
//
//   (cd <checkout> && npm run build -w @sidevoice/protocol -w @sidevoice/browser-audio -w @sidevoice/web)
//   node scripts/vendor-web.mjs <checkout>
//
// The heavy runtime files (ONNX Runtime and espeak-ng WebAssembly) are not vendored: scripts/web-assets.mjs
// copies them from this repo's pinned npm packages at build time.
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const source = process.argv[2];
if (!source) {
  console.error("usage: node scripts/vendor-web.mjs <rubasace/sidevoice checkout, built>");
  process.exit(2);
}
const web = path.join(source, "apps/web/dist");
const audio = path.join(source, "packages/browser-audio");
for (const needed of [path.join(web, "index.html"), path.join(audio, "dist/worker.js"), path.join(audio, "mic_capture.js")]) {
  if (!existsSync(needed)) {
    console.error(`missing ${needed}: build the checkout first`);
    process.exit(1);
  }
}

const voice = path.join(root, "ui/voice");
const voiceBrowser = path.join(root, "ui/voice-browser");
rmSync(voice, { recursive: true, force: true });
rmSync(voiceBrowser, { recursive: true, force: true });
cpSync(web, voice, { recursive: true });
cpSync(path.join(audio, "mic_capture.js"), path.join(voice, "mic_capture.js"));
mkdirSync(voiceBrowser, { recursive: true });
for (const entry of readdirSync(path.join(audio, "dist"))) {
  if (entry === "assets") continue; // runtime WebAssembly: scripts/web-assets.mjs
  cpSync(path.join(audio, "dist", entry), path.join(voiceBrowser, entry), { recursive: true });
}

const git = (...args) => execFileSync("git", ["-C", source, ...args], { encoding: "utf8" }).trim();
const record = {
  repository: "rubasace/sidevoice",
  branch: git("rev-parse", "--abbrev-ref", "HEAD"),
  commit: git("rev-parse", "HEAD"),
  uncommitted_changes: git("status", "--porcelain", "--", "apps/web", "packages/browser-audio") !== "",
  vendored_at: new Date().toISOString(),
};
writeFileSync(path.join(voice, "web-source.json"), JSON.stringify(record, null, 2) + "\n");
console.log(`vendored the web interface from ${record.branch}@${record.commit.slice(0, 7)}${record.uncommitted_changes ? " (with uncommitted changes)" : ""}`);
