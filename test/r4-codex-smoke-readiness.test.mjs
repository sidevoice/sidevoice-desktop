import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  assertCodexCliPath,
  assertCodexCliVersion,
  assertConnectedCodexReport,
  assertDisposableCodexHome,
  CODEX_CLI_VERSION,
} from "./macos/codex-smoke-readiness.mjs";

const smokeSource = readFileSync(new URL("./macos/native-pair-install-smoke.mjs", import.meta.url), "utf8");
const workflowSource = readFileSync(new URL("../.github/workflows/build.yml", import.meta.url), "utf8");

test("Mac app smoke accepts only the pinned Codex CLI in a disposable runner home", () => {
  assert.equal(CODEX_CLI_VERSION, "0.160.0");
  assert.equal(assertCodexCliVersion("codex-cli 0.160.0"), CODEX_CLI_VERSION);
  assert.throws(() => assertCodexCliVersion("codex-cli 0.159.0"), /not the pinned/);
  assert.equal(
    assertCodexCliPath("/tmp/runner/codex-cli/node_modules/.bin/codex", "/tmp/runner"),
    "/tmp/runner/codex-cli/node_modules/.bin/codex",
  );
  assert.throws(() => assertCodexCliPath("/usr/local/bin/codex", "/tmp/runner"), /runner-temp/);
  assert.equal(assertDisposableCodexHome("/tmp/runner/r4-codex-home", "/tmp/runner", "/Users/runner"),
    "/tmp/runner/r4-codex-home");
  assert.throws(() => assertDisposableCodexHome("/Users/runner/.codex", "/tmp/runner", "/Users/runner"), /isolated/);
  assert.throws(() => assertDisposableCodexHome("/tmp/elsewhere", "/tmp/runner", "/Users/runner"), /isolated/);
});

test("packaged install requires the SEA to report one connected Codex agent", () => {
  const row = { id: "codex", registration: "connected" };
  assert.equal(assertConnectedCodexReport({ agents: [row, { id: "claude", registration: "not-connected" }] }), row);
  for (const report of [
    { agents: [] },
    { agents: [{ id: "claude", registration: "connected" }] },
    { agents: [row, row] },
    { agents: [{ id: "codex", registration: "manual" }] },
    {},
  ]) {
    assert.throws(() => assertConnectedCodexReport(report), /Codex/);
  }

  assert.match(smokeSource, /"CODEX_HOME"/);
  assert.match(smokeSource, /"SIDEVOICE_CODEX_BIN"/);
  assert.match(smokeSource, /\["agents", "--json"\]/);
  assert.match(smokeSource, /assertConnectedCodexReport\(agentStatus\)/);
  assert.match(smokeSource, /rm\(codexHome,\s*\{ recursive: true, force: true \}\)/);
  assert.match(workflowSource, new RegExp(`R4_CODEX_CLI_VERSION:\\s*['"]?${CODEX_CLI_VERSION.replaceAll(".", "\\.")}['"]?`));
  assert.match(workflowSource, /@openai\/codex@\$R4_CODEX_CLI_VERSION/);
});
