// Explicit network-capable check of the real verifier CLI used in production; no genuine SEA is claimed here.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFile as execFileCallback } from "node:child_process";
import { copyFile, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { verifyConnectorAttestation } from "../scripts/connector-attestation.mjs";

const execFile = promisify(execFileCallback);

test("the real gh CLI accepts the production policy flags and rejects an unsigned bundle", async () => {
  const directory = await mkdtemp(join(tmpdir(), "sidevoice-gh-negative-"));
  try {
    const archive = join(directory, "sidevoice-provenance.zip");
    const executable = join(directory, "sidevoice");
    await copyFile(new URL("./fixtures/connector-provenance/invalid-bundle.zip", import.meta.url), archive);
    await writeFile(executable, "unsigned test subject", { mode: 0o600 });
    let verifierError;
    await assert.rejects(verifyConnectorAttestation(executable, archive, { connector_sha: "a".repeat(40) }, {
      // --bundle does not need a private API credential; trusted-root refresh uses public Sigstore infrastructure.
      token: "local-bundle-verification-only",
      run: async (file, args, options) => {
        try { return await execFile(file, args, options); }
        catch (error) { if (file === "gh") verifierError = error; throw error; }
      },
    }), /Sigstore verification failed/);
    assert.equal(verifierError?.code, 1);
    assert.match(verifierError.stderr, /bundle content could not be parsed/,
      "missing CLI, unsupported/mutually exclusive flags, auth or network failures are not verification coverage");
  } finally { await rm(directory, { recursive: true, force: true }); }
});
