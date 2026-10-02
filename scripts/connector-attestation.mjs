import { execFile as execFileCallback } from "node:child_process";
import { writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { promisify } from "node:util";
import { t } from "./build-i18n.mjs";

const execFile = promisify(execFileCallback);
const WORKFLOW = "github.com/sidevoice/sidevoice-connector/.github/workflows/r4-sea.yml";
const MAX_BUNDLE_BYTES = 1024 * 1024;

// Use GitHub's Sigstore verifier for certificate, transparency and SLSA subject validation.
// Never execute a downloaded SEA to decide whether its publisher is trusted.
export async function verifyConnectorAttestation(executable, archive, pin, {
  token, run = execFile,
} = {}) {
  const bundle = resolve(archive, "..", "sidevoice.sigstore.json");
  try {
    const { stdout: names } = await run("unzip", ["-Z1", archive], {
      timeout: 20_000, maxBuffer: MAX_BUNDLE_BYTES,
    });
    if (names.trim() !== "sidevoice.sigstore.json") throw new Error();
    const { stdout: bytes } = await run("unzip", ["-p", archive, "sidevoice.sigstore.json"], {
      timeout: 20_000, maxBuffer: MAX_BUNDLE_BYTES, encoding: "buffer",
    });
    if (!bytes.length) throw new Error();
    await writeFile(bundle, bytes, { flag: "wx", mode: 0o600 });
  } catch { throw new Error(t("pin.attestationArchive")); }
  try {
    await run("gh", ["attestation", "verify", executable,
      "--repo", "sidevoice/sidevoice-connector", "--bundle", bundle,
      "--cert-oidc-issuer", "https://token.actions.githubusercontent.com",
      "--cert-identity", `https://${WORKFLOW}@refs/heads/main`,
      "--signer-digest", pin.connector_sha,
      "--source-ref", "refs/heads/main", "--source-digest", pin.connector_sha,
      "--predicate-type", "https://slsa.dev/provenance/v1", "--deny-self-hosted-runners",
    ], {
      timeout: 120_000, maxBuffer: MAX_BUNDLE_BYTES,
      env: { ...process.env, ...(token ? { GH_TOKEN: token } : {}), GH_HOST: "github.com", GH_PROMPT_DISABLED: "1" },
    });
  } catch { throw new Error(t("pin.attestationFailed")); }
}
