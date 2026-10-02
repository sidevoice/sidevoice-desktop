import { test } from "node:test";
import assert from "node:assert/strict";
import { sha256, validatePin, verifyMetadata } from "../scripts/connector-pin.mjs";

function fixturePin() {
  const manifest = Buffer.from('{"version":"0.9.0","bundles":[]}');
  return {
    schema: 1,
    status: "ready",
    target: "macos-aarch64",
    connector_sha: "a".repeat(40),
    connector_version: "1.2.3",
    channel: "nightly",
    build_seq: 42,
    core_version: "0.9.0",
    core_manifest_sha256: sha256(manifest),
    core_manifest_size: manifest.length,
    core_manifest_bytes_base64: manifest.toString("base64"),
    core_assets: [{ name: "core.tar.zst", url: "https://github.com/sidevoice/sidevoice-core/releases/download/v0.9.0/core.tar.zst",
      sha256: "b".repeat(64), size: 10 }],
    core_manifest_sidecars: [{ name: "manifest.sigstore.json",
      url: "https://github.com/sidevoice/sidevoice-core/releases/download/v0.9.0/manifest.sigstore.json",
      sha256: "c".repeat(64), size: 11 }],
    executable_sha256: "d".repeat(64),
    executable_size: 123,
    asset_url: "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/runs/123/artifacts/456/zip",
    metadata_protocol: "sidevoice-metadata-v1",
    progress_protocol: "sidevoice-progress-jsonl-v1",
    core_api: 1,
    core_link: 1,
    link_min: 1,
    link_max: 1,
    provenance: {
      repository: "sidevoice/sidevoice-connector",
      repository_id: "12345",
      workflow: ".github/workflows/build.yml@refs/heads/main",
      run_id: 123,
      artifact_name: "sidevoice-macos-aarch64",
      sidecars: [{ name: "sidevoice.sigstore.json",
        url: "https://github.com/sidevoice/sidevoice-connector/releases/download/v1.2.3/sidevoice.sigstore.json",
        sha256: "e".repeat(64), size: 12 }],
    },
  };
}

test("production pin validation rejects the pending pin and the PR manifest-less SEA", () => {
  assert.throws(() => validatePin({ schema: 1, status: "pending" }), /pending R4-b/);
  const pin = fixturePin();
  pin.core_manifest_bytes_base64 = Buffer.from("{}").toString("base64");
  assert.throws(() => validatePin(pin), /manifest bytes do not match/);
  const wrongVersion = fixturePin();
  const manifest = Buffer.from('{"version":"0.8.0","bundles":[]}');
  wrongVersion.core_manifest_bytes_base64 = manifest.toString("base64");
  wrongVersion.core_manifest_size = manifest.length;
  wrongVersion.core_manifest_sha256 = sha256(manifest);
  assert.throws(() => validatePin(wrongVersion), /manifest version does not match/);
});

test("a test-only pin requires immutable artifacts, manifest bytes, attestation sidecars and protocols", () => {
  const pin = fixturePin();
  assert.equal(validatePin(pin), pin);
  pin.asset_url = "https://github.com/sidevoice/sidevoice-connector/releases/download/nightly/sidevoice";
  assert.throws(() => validatePin(pin), /immutable connector artifact/);
  const malformed = fixturePin();
  malformed.asset_url = malformed.asset_url.replace("/artifacts/456/", "/artifacts/current/");
  assert.throws(() => validatePin(malformed), /immutable connector artifact/);
});

test("SEA version and embedded core metadata must match the pin", () => {
  const pin = fixturePin();
  const version = { ok: true, version: pin.connector_version, target: pin.target, channel: pin.channel,
    connector_sha: pin.connector_sha, build_seq: pin.build_seq };
  const metadata = { ok: true,
    connector: { version: pin.connector_version, sha: pin.connector_sha, channel: pin.channel, build_seq: pin.build_seq,
      link_min: pin.link_min, link_max: pin.link_max },
    embedded_core: { version: pin.core_version, manifest_sha256: pin.core_manifest_sha256,
      api: pin.core_api, link: pin.core_link },
    protocols: { metadata: pin.metadata_protocol, progress: pin.progress_protocol } };
  assert.equal(verifyMetadata(pin, version, metadata), true);
  metadata.embedded_core.manifest_sha256 = "0".repeat(64);
  assert.throws(() => verifyMetadata(pin, version, metadata), /embedded_core.manifest_sha256/);
});
