import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const hostSource = readFileSync(new URL("../src-tauri/local-host/src/host.rs", import.meta.url), "utf8");

test("packaged install asks the transactional Connector to register only Codex", () => {
  const declaration = hostSource.match(/const PACKAGED_INSTALL_ARGS:\s*&\[&str\]\s*=\s*&\[([^\]]*)\];/);
  assert.ok(declaration, "the packaged installer arguments stay explicit and reviewable");
  const args = [...declaration[1].matchAll(/"([^\"]+)"/g)].map((match) => match[1]);
  assert.deepEqual(args, ["install", "--harness", "codex", "--service", "--json", "--progress=jsonl"]);
  assert.match(hostSource, /cli\.run_with_progress\(\s*PACKAGED_INSTALL_ARGS\s*,\s*INSTALL_TIMEOUT/);
  assert.doesNotMatch(hostSource, /--no-agents/);
});
