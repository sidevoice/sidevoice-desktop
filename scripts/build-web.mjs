#!/usr/bin/env node
// Puts the Sidevoice web interface the app bundles in ui/: the released static site of sidevoice-web that
// `web.pin.json` names (a release tag and its tarball's SHA-256), downloaded, checked and unpacked by
// web-release.mjs, then copied to ui/ at the paths a room serves it from (/voice/), by web-site.mjs. Nothing of it is
// committed here: tauri.conf.json runs this before every build and dev run, and CI before the tests that read it.
//
//   node scripts/build-web.mjs
//
// The unpacked site is kept under target/web/<sha256>/site, so a second run at the same pin only copies. The release's
// attestation is verified with `gh attestation verify` when `gh` is on the PATH; with SIDEVOICE_WEB_ATTESTATION=require
// (CI), a missing `gh` or a failed verification stops the build.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { fetchSite, ghAttest, readPin } from "./web-release.mjs";
import { installSite } from "./web-site.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
let pin;
try {
  pin = readPin(JSON.parse(readFileSync(path.join(root, "web.pin.json"), "utf8")));
} catch (error) {
  console.error(error.message);
  process.exit(1);
}

const hasGh = (() => {
  try {
    execFileSync("gh", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
})();
const required = process.env.SIDEVOICE_WEB_ATTESTATION === "require";
if (required && !hasGh) {
  console.error("SIDEVOICE_WEB_ATTESTATION=require, and gh is not on the PATH to verify the release's attestation");
  process.exit(1);
}
if (!hasGh) console.warn(`gh is not on the PATH: ${pin.tag}'s attestation is not verified, only its SHA-256`);

const site = await fetchSite(pin, { work: path.join(root, "target/web", pin.sha256), attest: hasGh ? ghAttest : null });
const ui = path.join(root, "ui");
const served = installSite(site, ui);
if (!served.includes("voice")) {
  console.error(`${pin.repository} ${pin.tag} has no voice/ page`);
  process.exit(1);
}
writeFileSync(path.join(ui, "voice/web-source.json"), JSON.stringify(pin, null, 2) + "\n");
console.log(`web interface ${pin.repository} ${pin.tag} (${pin.sha256.slice(0, 12)}) into ${served.map((name) => `ui/${name}/`).join(", ")}`);
