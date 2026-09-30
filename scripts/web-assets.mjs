#!/usr/bin/env node
// Puts the in-browser models' runtime files where the bundled interface looks for them
// (/voice-browser/assets/, see packages/browser-audio in rubasace/sidevoice), from the npm packages pinned in
// this repo's package.json — the same versions the room serves. Run before every build (tauri.conf.json
// beforeBuildCommand); the files are not committed.
import { copyFileSync, mkdirSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const out = path.join(root, "ui/voice-browser/assets");
mkdirSync(out, { recursive: true });

const espeak = path.join(path.dirname(require.resolve("espeak-ng/package.json")), "dist");
for (const name of ["espeak-ng.js", "espeak-ng.wasm"]) copyFileSync(path.join(espeak, name), path.join(out, name));

// Only the WebAssembly runtime and its loaders: the library itself is bundled into the workers.
const ort = path.dirname(require.resolve("onnxruntime-web"));
let copied = 0;
for (const name of readdirSync(ort)) {
  if (/^ort-wasm-simd-threaded(\.jsep)?\.(wasm|mjs)$/.test(name)) {
    copyFileSync(path.join(ort, name), path.join(out, name));
    copied++;
  }
}
if (copied < 4) {
  console.error(`expected 4 ONNX Runtime files in ${ort}, found ${copied}`);
  process.exit(1);
}
console.log(`web assets ready in ${path.relative(root, out)}`);
