#!/usr/bin/env node
// Builds the Sidevoice web interface the app bundles, from sidevoice-web at the commit `web.pin.json` names, and
// puts it in ui/ at the paths a room serves it from (/voice/, /voice-browser/: that repo's
// scripts/assemble-static-web.mjs defines the layout). Nothing of it is committed here: tauri.conf.json runs this
// before every build and dev run, and CI before the tests that read it.
//
//   node scripts/build-web.mjs
//
// The built site is kept under target/web/<commit>/site, so a second run at the same pin only copies.
// `ref` in the pin is a reminder for people (the branch or tag the commit came from); the commit is what is built,
// and the checkout must be exactly it.
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const pin = JSON.parse(readFileSync(path.join(root, "web.pin.json"), "utf8"));
if (!/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(pin.repository ?? "") || !/^[0-9a-f]{40}$/.test(pin.commit ?? "")) {
  console.error("web.pin.json must name a GitHub repository (owner/name) and a full 40-hex commit");
  process.exit(1);
}

const work = path.join(root, "target/web", pin.commit);
const checkout = path.join(work, "source");
const site = path.join(work, "site");
const done = path.join(work, ".built");
// npm is a .cmd on Windows, which only a shell runs.
const run = (command, args, cwd) =>
  execFileSync(command, args, { cwd, stdio: "inherit", shell: process.platform === "win32" && command === "npm" });
const git = (...args) => execFileSync("git", ["-C", checkout, ...args], { encoding: "utf8" }).trim();

if (!existsSync(done)) {
  rmSync(work, { recursive: true, force: true });
  mkdirSync(checkout, { recursive: true });
  run("git", ["init", "--quiet"], checkout);
  run("git", ["fetch", "--quiet", "--depth", "1", `https://github.com/${pin.repository}.git`, pin.commit], checkout);
  run("git", ["checkout", "--quiet", "--detach", "FETCH_HEAD"], checkout);
  if (git("rev-parse", "HEAD") !== pin.commit) {
    console.error(`fetched ${git("rev-parse", "HEAD")}, the pin says ${pin.commit}`);
    process.exit(1);
  }
  run("npm", ["ci", "--no-audit", "--no-fund"], checkout);
  run("npm", ["run", "build"], checkout);
  run("node", [path.join(checkout, "scripts/assemble-static-web.mjs"), site], checkout);
  // Only the site is kept: the checkout and its dependencies are a gigabyte.
  rmSync(checkout, { recursive: true, force: true });
  writeFileSync(done, "");
}

// Everything the site serves below its root goes to ui/ at the same path; its root index.html (a redirect to
// /voice/) does not: the app opens /voice/index.html itself.
const ui = path.join(root, "ui");
const served = readdirSync(site, { withFileTypes: true }).filter((entry) => entry.isDirectory());
if (!served.some((entry) => entry.name === "voice")) {
  console.error(`${pin.repository}@${pin.commit.slice(0, 7)} built no voice/ page`);
  process.exit(1);
}
for (const entry of served) {
  rmSync(path.join(ui, entry.name), { recursive: true, force: true });
  cpSync(path.join(site, entry.name), path.join(ui, entry.name), { recursive: true });
}
const record = { repository: pin.repository, ref: pin.ref ?? null, commit: pin.commit };
writeFileSync(path.join(ui, "voice/web-source.json"), JSON.stringify(record, null, 2) + "\n");
console.log(`web interface ${pin.repository}@${pin.commit.slice(0, 7)} built into ${served.map((e) => `ui/${e.name}/`).join(", ")}`);
