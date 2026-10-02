import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { verifyArtifact } from "./connector-pin.mjs";

const executable = process.argv[2];
if (!executable) throw new Error("Pass the packaged resources/sidevoice path.");
const pin = JSON.parse(await readFile(resolve("src-tauri/connector-pin.json"), "utf8"));
const result = await verifyArtifact(executable, pin);
process.stdout.write(`${JSON.stringify({ size: result.size, sha256: result.sha256, version: result.version, metadata: result.metadata })}\n`);
