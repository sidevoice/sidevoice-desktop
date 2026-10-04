import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const hostSource = readFileSync(new URL("../src-tauri/local-host/src/host.rs", import.meta.url), "utf8");
const appHostSource = readFileSync(new URL("../src-tauri/src/local_host.rs", import.meta.url), "utf8");

test("packaged install asks the transactional Connector to register only Codex", () => {
  const declaration = hostSource.match(/const PACKAGED_INSTALL_ARGS:\s*&\[&str\]\s*=\s*&\[([^\]]*)\];/);
  assert.ok(declaration, "the packaged installer arguments stay explicit and reviewable");
  const args = [...declaration[1].matchAll(/"([^\"]+)"/g)].map((match) => match[1]);
  assert.deepEqual(args, ["install", "--harness", "codex", "--service", "--json", "--progress=jsonl"]);
  assert.match(hostSource, /cli\.run_with_progress\(\s*PACKAGED_INSTALL_ARGS\s*,\s*INSTALL_TIMEOUT/);
  assert.doesNotMatch(hostSource, /--no-agents/);
});

test("packaged install and current-pair no-op require a confirmed detected Codex", () => {
  assert.match(hostSource, /run_json\(\s*&\["agents", "--json"\]/);
  assert.match(hostSource, /codex_registration_status\(&answer\)/);
  assert.match(hostSource, /ensure_codex_registration\(&cli\)\?/);
  assert.match(hostSource, /ensure_codex_registration\(cli\)\?/);
  assert.match(appHostSource, /UpdateStatus::Available \| UpdateStatus::Current/);
  assert.match(appHostSource, /reconcile_current = versioning::update_status\(Some\(&pin\), installed\.as_ref\(\)\) == UpdateStatus::Current/);
});
