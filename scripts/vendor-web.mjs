#!/usr/bin/env node
// Copies the Sidevoice web interface from a sidevoice/sidevoice-web checkout into ui/, in the layout that repo
// defines for a static deployment (its scripts/assemble-static-web.mjs: /voice/, /voice-browser/), so the app
// serves exactly what a standalone static site serves. Records where it came from in ui/voice/web-source.json.
//
//   (cd <checkout> && npm run build -w @sidevoice/protocol && npm run build -w @sidevoice/browser-audio && npm run build -w @sidevoice/web)
//   node scripts/vendor-web.mjs <checkout>
//
// The heavy runtime files (ONNX Runtime and espeak-ng WebAssembly, /voice-browser/assets/) are not vendored:
// scripts/web-assets.mjs copies them from this repo's pinned npm packages at build time.
import { cpSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const source = process.argv[2];
if (!source) {
  console.error("usage: node scripts/vendor-web.mjs <sidevoice/sidevoice-web checkout, built>");
  process.exit(2);
}
const site = mkdtempSync(path.join(os.tmpdir(), "sidevoice-site-"));
execFileSync("node", [path.join(source, "scripts/assemble-static-web.mjs"), site], { stdio: "inherit" });

const voice = path.join(root, "ui/voice");
const voiceBrowser = path.join(root, "ui/voice-browser");
rmSync(voice, { recursive: true, force: true });
mkdirSync(voiceBrowser, { recursive: true });
for (const entry of readdirSync(voiceBrowser, { withFileTypes: true }).filter((e) => e.name !== "assets")) {
  rmSync(path.join(voiceBrowser, entry.name), { recursive: true, force: true });
}
cpSync(path.join(site, "voice"), voice, { recursive: true });
for (const entry of readdirSync(path.join(site, "voice-browser"))) {
  if (entry === "assets") continue; // runtime WebAssembly: scripts/web-assets.mjs
  cpSync(path.join(site, "voice-browser", entry), path.join(voiceBrowser, entry), { recursive: true });
}
rmSync(site, { recursive: true, force: true });

const git = (...args) => execFileSync("git", ["-C", source, ...args], { encoding: "utf8" }).trim();
// owner/name from the checkout's own remote, never assumed: the record says where the bytes came from.
const remote = git("remote", "get-url", "origin");
const repository = remote.replace(/\.git$/, "").match(/[/:]([^/:]+\/[^/]+)$/)?.[1];
if (!repository) throw new Error(`cannot tell the repository from the remote ${remote}`);
const record = {
  repository,
  branch: git("rev-parse", "--abbrev-ref", "HEAD"),
  commit: git("rev-parse", "HEAD"),
  uncommitted_changes: git("status", "--porcelain", "--", "apps/web", "packages/browser-audio") !== "",
  vendored_at: new Date().toISOString(),
};
writeFileSync(path.join(voice, "web-source.json"), JSON.stringify(record, null, 2) + "\n");
console.log(`vendored the web interface from ${record.branch}@${record.commit.slice(0, 7)}${record.uncommitted_changes ? " (with uncommitted changes)" : ""}`);
