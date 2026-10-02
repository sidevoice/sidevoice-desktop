import { createHash } from "node:crypto";
import { execFile as execFileCallback } from "node:child_process";
import { promisify } from "node:util";
import { readFile, stat } from "node:fs/promises";

const execFile = promisify(execFileCallback);
const FIXTURE_ID = "sidevoice-r4-c-package-fixture-v1";

export const fixtureResponses = Object.freeze({
  version: Object.freeze({ ok: true, fixture: FIXTURE_ID }),
  metadata: Object.freeze({ ok: true, fixture: FIXTURE_ID }),
});

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export async function verifyConnectorFixture(executable, expectedPath, pinPath) {
  const [pinBytes, expectedBytes, executableBytes, info] = await Promise.all([
    readFile(pinPath),
    readFile(expectedPath),
    readFile(executable),
    stat(executable),
  ]);
  const pin = JSON.parse(pinBytes);
  const expected = JSON.parse(expectedBytes);
  if (pin.schema !== 1 || pin.status !== "pending" || pin.target !== "macos-aarch64") {
    throw new Error("The package fixture requires the checked-in pending macOS arm64 production pin.");
  }
  if (expected.fixture !== FIXTURE_ID || expected.size !== info.size || expected.sha256 !== sha256(executableBytes)) {
    throw new Error("The packaged connector fixture does not match the CI-prepared fixture identity.");
  }
  const [version, metadata] = await Promise.all([
    execFile(executable, ["--version", "--json"], { timeout: 5_000, maxBuffer: 1024 * 1024 }),
    execFile(executable, ["metadata", "--json"], { timeout: 5_000, maxBuffer: 1024 * 1024 }),
  ]);
  const actual = { version: JSON.parse(version.stdout), metadata: JSON.parse(metadata.stdout) };
  if (JSON.stringify(actual) !== JSON.stringify(fixtureResponses)) {
    throw new Error("The packaged connector fixture did not return its fixed version and metadata responses.");
  }
  if (JSON.stringify(expected.responses) !== JSON.stringify(fixtureResponses)) {
    throw new Error("The CI-prepared connector fixture expectation is malformed.");
  }
  return { size: info.size, sha256: expected.sha256, fixture: FIXTURE_ID };
}
