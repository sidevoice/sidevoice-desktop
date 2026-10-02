import { test } from "node:test";
import assert from "node:assert/strict";
import { chmod, copyFile, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
