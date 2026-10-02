import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import assert from "node:assert/strict";
import { chmod, copyFile, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fixtureResponses, sha256, verifyConnectorFixture } from "../scripts/connector-package-fixture.mjs";

test("the explicit pending-pin package fixture verifies its executable identity and fixed CLI responses", async () => {
  const directory = await mkdtemp(resolve(tmpdir(), "sidevoice-connector-fixture-"));
  try {
    const executable = resolve(directory, "sidevoice");
    const expectedPath = resolve(directory, "expected.json");
    const pinPath = resolve(directory, "connector-pin.json");
    await copyFile(resolve("test/fixtures/connector-package-cli.sh"), executable);
    await chmod(executable, 0o755);
    const bytes = await readFile(executable);
    await writeFile(pinPath, JSON.stringify({ schema: 1, status: "pending", target: "macos-aarch64" }));
    await writeFile(expectedPath, JSON.stringify({ fixture: "sidevoice-r4-c-package-fixture-v1", size: bytes.length,
      sha256: sha256(bytes), responses: fixtureResponses }));
    assert.equal((await verifyConnectorFixture(executable, expectedPath, pinPath)).fixture,
      "sidevoice-r4-c-package-fixture-v1");

    await writeFile(executable, `${await readFile(executable, "utf8")}# changed\n`);
    await assert.rejects(verifyConnectorFixture(executable, expectedPath, pinPath), /does not match.*identity/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});


test("fixture preparation can repeat but cannot replace an existing different resource", async () => {
  const directory = await mkdtemp(resolve(tmpdir(), "sidevoice-fixture-prepare-"));
  try {
    await mkdir(resolve(directory, "src-tauri"));
    await mkdir(resolve(directory, "test/fixtures"), { recursive: true });
    await writeFile(resolve(directory, "src-tauri/connector-pin.json"), JSON.stringify({ status: "pending" }));
    await copyFile(resolve("test/fixtures/connector-package-cli.sh"), resolve(directory, "test/fixtures/connector-package-cli.sh"));
    const prepare = fileURLToPath(new URL("../scripts/prepare-ci-connector.mjs", import.meta.url));
    const run = (name) => execFileSync(process.execPath, [prepare, resolve(directory, name)],
      { cwd: directory, env: { ...process.env, GITHUB_OUTPUT: "" }, stdio: "pipe" });
    run("first.json"); run("second.json");
    assert.deepEqual(JSON.parse(await readFile(resolve(directory, "first.json"))),
      JSON.parse(await readFile(resolve(directory, "second.json"))));
    const resource = resolve(directory, "src-tauri/resources/sidevoice");
    await writeFile(resource, "existing real input");
    assert.throws(() => run("third.json"), /refusing to replace/);
    assert.equal(await readFile(resource, "utf8"), "existing real input");
  } finally { await rm(directory, { recursive: true, force: true }); }
});
