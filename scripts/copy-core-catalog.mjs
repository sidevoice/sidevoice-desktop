#!/usr/bin/env node
// Copies the model catalog from a sidevoice/sidevoice-core checkout into catalog/engines.json, byte for byte. The
// core owns it (rubasace/sidevoice#124 §3); this copy is what the app bundles, and it is never edited here.
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
const from = path.join(core, "src", "sidevoice_core", "models", "catalog.json");
if (!existsSync(from)) {
  console.error(`${from} does not exist: is ${core} a sidevoice-core checkout?`);
  process.exit(1);
}
copyFileSync(from, path.join(root, "catalog", "engines.json"));
console.log("catalog/engines.json ← sidevoice-core models/catalog.json");
