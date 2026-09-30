#!/usr/bin/env node
// Copies the model catalog and its resolver vectors from a sidevoice/sidevoice-core checkout into catalog/, byte for
// byte. The core owns both (rubasace/sidevoice#124 §3); these copies are what the app bundles and what
// src-tauri/core/src/engines.rs is tested against, and they are never edited here.
//
//   node scripts/copy-core-catalog.mjs <sidevoice-core checkout>
import { copyFileSync, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const core = process.argv[2];
if (!core) {
  console.error("usage: node scripts/copy-core-catalog.mjs <sidevoice-core checkout>");
  process.exit(2);
}
const source = path.join(core, "src", "sidevoice_core", "models");
for (const [from, to] of [["catalog.json", "engines.json"], ["vectors.json", "vectors.json"]]) {
  if (!existsSync(path.join(source, from))) {
    console.error(`${path.join(source, from)} does not exist: is ${core} a sidevoice-core checkout?`);
    process.exit(1);
  }
  copyFileSync(path.join(source, from), path.join(root, "catalog", to));
  console.log(`catalog/${to} ← sidevoice-core models/${from}`);
}
