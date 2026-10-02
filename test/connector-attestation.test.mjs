import { test } from "node:test";
import assert from "node:assert/strict";
import { execFile as execFileCallback } from "node:child_process";
import { copyFile, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { verifyConnectorAttestation } from "../scripts/connector-attestation.mjs";

const execFile = promisify(execFileCallback);
const pin = { connector_sha: "a".repeat(40) };

async function fixture(name, fn) {
  const directory = await mkdtemp(join(tmpdir(), "sidevoice-attestation-test-"));
  try {
    const archive = join(directory, "sidevoice-provenance.zip");
    await copyFile(new URL(`./fixtures/connector-provenance/${name}.zip`, import.meta.url), archive);
    const executable = join(directory, "sidevoice");
    await writeFile(executable, "untrusted SEA test bytes", { mode: 0o600 });
    await fn({ directory, archive, executable });
  } finally { await rm(directory, { recursive: true, force: true }); }
}

test("unsafe, duplicate, empty and oversized provenance archives fail before signature verification", async () => {
  await execFile("unzip", ["-v"]); // A missing tool must not masquerade as archive rejection coverage.
  for (const name of ["traversal", "extra", "duplicate", "empty", "oversized"]) {
    await fixture(name, async ({ archive, executable }) => {
      await assert.rejects(verifyConnectorAttestation(executable, archive, pin, { run: (file, args, options) => {
        assert.equal(file, "unzip", "invalid archives must never reach gh or execute the SEA");
        return execFile(file, args, options);
      } }), /provenance archive/);
    });
  }
});

test("GitHub's verifier gets the exact subject and main signer policy, and failures are fatal and sanitized", async () => {
  await fixture("invalid-bundle", async ({ directory, archive, executable }) => {
    let verified = false;
    await assert.rejects(verifyConnectorAttestation(executable, archive, pin, {
      token: "test-token",
      run: async (file, args, options) => {
        if (file === "unzip") return execFile(file, args, options);
        assert.equal(file, "gh");
        assert.deepEqual(args.slice(0, 3), ["attestation", "verify", executable]);
        const flag = (key) => args[args.indexOf(key) + 1];
        assert.equal(flag("--repo"), "sidevoice/sidevoice-connector");
        assert.equal(flag("--bundle"), join(directory, "sidevoice.sigstore.json"));
        assert.equal(flag("--cert-oidc-issuer"), "https://token.actions.githubusercontent.com");
        assert.equal(flag("--cert-identity"), "https://github.com/sidevoice/sidevoice-connector/.github/workflows/r4-sea.yml@refs/heads/main");
        assert.ok(!args.includes("--signer-workflow"), "gh forbids combining this with exact --cert-identity");
        assert.equal(flag("--signer-digest"), pin.connector_sha);
        assert.equal(flag("--source-digest"), pin.connector_sha);
        assert.equal(flag("--source-ref"), "refs/heads/main");
        assert.equal(flag("--predicate-type"), "https://slsa.dev/provenance/v1");
        assert.ok(args.includes("--deny-self-hosted-runners"));
        assert.equal(options.env.GH_TOKEN, "test-token");
        assert.equal(options.env.GH_HOST, "github.com");
        assert.equal((await stat(executable)).mode & 0o111, 0, "no execute permission before provenance is verified");
        assert.equal(await readFile(flag("--bundle"), "utf8"), '{"invalid":"not a signed bundle"}');
        verified = true;
        // This is the verifier boundary, not a fake successful signature. Production uses the real gh binary.
        throw new Error("untrusted signature; private-token and signed-url-secret must not be logged");
      },
    }), (error) => /Sigstore verification failed/.test(error.message) && !/private-token|signed-url-secret/.test(error.message));
    assert.equal(verified, true);
  });
});
