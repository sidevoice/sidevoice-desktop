import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(".github/workflows/build.yml", "utf8");
const macStart = workflow.indexOf("\n  macos:\n");
const engineStart = workflow.indexOf("\n  engine:\n", macStart);
assert.notEqual(macStart, -1);
assert.notEqual(engineStart, -1);
const macJob = workflow.slice(macStart, engineStart);
const archiveScript = "scripts/archive-mac-preview.sh";
const archiveSource = readFileSync(archiveScript, "utf8");
const buildSource = readFileSync("scripts/build-mac.mjs", "utf8");

test("macOS package can start with the path filter, independent of lint/unit completion", () => {
  assert.match(macJob, /needs: \[changes\]/);
  assert.doesNotMatch(macJob, /needs: \[changes, test\]/);

  const nativeFilter = workflow.slice(workflow.indexOf("            native:\n"), workflow.indexOf("            engine:\n"));
  for (const path of ["ui/index.html", "ui/settings.js", "ui/settings.css", "ui/i18n.js", "ui/i18n/**"]) {
    assert.ok(nativeFilter.includes(`- '${path}'`), `native path filter must include ${path}`);
  }
});

test("verified production app is archived and uploaded before any probe build", () => {
  const buildAt = macJob.indexOf("- name: Build and verify the real app");
  const archiveAt = macJob.indexOf("- name: Archive verified development app");
  const uploadAt = macJob.indexOf("- name: Upload development app preview");
  const probeAt = macJob.indexOf("- name: Build the CI probe app");
  assert.ok(buildAt < archiveAt && archiveAt < uploadAt && uploadAt < probeAt);

  const productionCondition = "(github.event_name != 'pull_request' && !inputs.fixture) || steps.connector.outputs.status != 'pending'";
  for (const section of [
    macJob.slice(buildAt, archiveAt),
    macJob.slice(archiveAt, uploadAt),
    macJob.slice(uploadAt, probeAt),
  ]) {
    assert.ok(section.includes(`if: ${productionCondition}`));
  }

  const publishAt = workflow.indexOf("\n  publish:\n");
  assert.match(workflow.slice(publishAt), /needs: \[changes, test, macos, engine, linux, windows\]/);
  assert.match(
    macJob.slice(uploadAt, probeAt),
    /name: development-macos-arm64-app-\$\{\{ github\.event\.pull_request\.head\.sha \|\| github\.sha \}\}/,
  );
  assert.match(buildSource, /desktop_sha: git\("rev-parse", "HEAD"\)/);
  assert.match(macJob.slice(uploadAt, probeAt), /compression-level: 0/);
  assert.match(macJob.slice(uploadAt, probeAt), /retention-days: 7/);
  assert.match(macJob.slice(uploadAt, probeAt), /if-no-files-found: error/);
  assert.doesNotMatch(macJob.slice(uploadAt, probeAt), /name: Sidevoice-/);
  for (const path of [
    "src-tauri/target/previews/Sidevoice-dev-macos-arm64.zip",
    "src-tauri/target/previews/Sidevoice-dev-macos-arm64.zip.sha256",
    "dist/build-evidence.json",
  ]) {
    assert.ok(macJob.slice(uploadAt, probeAt).includes(path));
  }
});

test("archive script is executable, shell-valid, and checks the signed evidence round trip", () => {
  assert.ok((statSync(archiveScript).mode & 0o111) !== 0, "archive script must be executable");
  const syntax = spawnSync("bash", ["-n", archiveScript], { encoding: "utf8" });
  assert.equal(syntax.status, 0, syntax.stderr);
  for (const contract of [
    "src-tauri/target/packages/production/Sidevoice.app",
    "dist/build-evidence.json",
    "ditto -c -k --sequesterRsrc --keepParent",
    'SIDECAR_NAME="$ARCHIVE_NAME.sha256"',
    "ditto -x -k",
    "codesign --verify --deep --strict",
    "app_executable_sha256",
    "Archived app executable does not match build evidence",
  ]) {
    assert.ok(archiveSource.includes(contract), `archive script must enforce ${contract}`);
  }
});
