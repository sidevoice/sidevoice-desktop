#!/usr/bin/env node
// Copies the Sidevoice brand files this app is drawn from, out of a sidevoice/brand-resources checkout,
// into brand/, and records where they came from in brand/source.json. The brand repo owns them: change
// them there, then run this again and `npm run icons`.
//
//   node scripts/vendor-brand.mjs <brand-resources checkout>
import { copyFileSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const FILES = [
  "logo/mark/sidevoice-mark.svg",
  "logo/mark/sidevoice-mark-on-dark.svg",
  "logo/mark/sidevoice-mark-black.svg",
  "logo/mark/sidevoice-mark-white.svg",
  "logo/tile/sidevoice-tile.svg",
  "logo/lockup/sidevoice-lockup.svg",
  "logo/lockup/sidevoice-lockup-on-dark.svg",
  "color/palette.json",
  "fonts/DMSans-Variable-latin.woff2",
  "fonts/OFL.txt",
];

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const source = process.argv[2];
if (!source) {
  console.error("usage: node scripts/vendor-brand.mjs <sidevoice/brand-resources checkout>");
  process.exit(2);
}
const brand = path.join(root, "brand");
rmSync(brand, { recursive: true, force: true });
for (const file of FILES) {
  mkdirSync(path.dirname(path.join(brand, file)), { recursive: true });
  copyFileSync(path.join(source, file), path.join(brand, file));
}
const git = (...args) => execFileSync("git", ["-C", source, ...args], { encoding: "utf8" }).trim();
const record = {
  repository: "sidevoice/brand-resources",
  commit: git("rev-parse", "HEAD"),
  uncommitted_changes: git("status", "--porcelain", "--", ...FILES) !== "",
  files: FILES,
};
writeFileSync(path.join(brand, "source.json"), JSON.stringify(record, null, 2) + "\n");
console.log(`vendored ${FILES.length} brand files from ${record.repository}@${record.commit.slice(0, 7)}`);
