import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { sha256, validatePin, verifyMetadata } from "../scripts/connector-pin.mjs";

function producerManifest() {
  return readFileSync(new URL("../src-tauri/local-host/test-fixtures/core-manifest-core34.json", import.meta.url));
}

function fixturePin() {
  const manifest = producerManifest();
  const manifestJson = JSON.parse(manifest);
  return {
    schema: 1,
    status: "ready",
    target: "macos-aarch64",
    connector_sha: "a".repeat(40),
    connector_version: "1.2.3",
    channel: "nightly",
    build_seq: 42,
    core_version: "0.1.0",
    core_manifest_sha256: sha256(manifest),
    core_manifest_size: manifest.length,
    core_manifest_bytes_base64: manifest.toString("base64"),
    core_assets: manifestJson.bundles.map((asset) => ({
      name: asset.url.split("/").at(-1), url: asset.url, sha256: asset.sha256, size: asset.size,
    })),
    core_manifest_sidecars: [{ name: "manifest.sigstore.json",
      url: "https://github.com/sidevoice/sidevoice-core/releases/download/v0.1.0/manifest.sigstore.json",
      sha256: "c".repeat(64), size: 11 }],
    executable_sha256: "d".repeat(64),
    executable_size: 123,
    asset_url: "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/456/zip",
    metadata_protocol: "sidevoice-metadata-v1",
    progress_protocol: "sidevoice-progress-jsonl-v1",
    core_api: 1,
    core_link: 1,
    link_min: 1,
    link_max: 1,
    provenance: {
      repository: "sidevoice/sidevoice-connector",
      repository_id: "12345",
      workflow: ".github/workflows/r4-sea.yml@refs/heads/main",
      run_id: 123,
      artifact_name: "sidevoice-macos-aarch64",
      sidecars: [{ name: "sidevoice.sigstore.json",
        url: "https://github.com/sidevoice/sidevoice-connector/releases/download/v1.2.3/sidevoice.sigstore.json",
        sha256: "e".repeat(64), size: 12 }],
    },
  };
}

test("production pin accepts core PR #34 producer bytes and rejects pending or manifest-less pins", () => {
  assert.throws(() => validatePin({ schema: 1, status: "pending" }), /pending R4-b/);
  const pin = fixturePin();
  assert.equal(validatePin(pin), pin, "real producer bytes have bundles and wheel, with no synthetic version field");
  pin.core_manifest_bytes_base64 = Buffer.from("{}").toString("base64");
  assert.throws(() => validatePin(pin), /manifest bytes do not match/);
  const manifestVersion = fixturePin();
  const withSyntheticVersion = Buffer.from(JSON.stringify({ version: "0.1.0", ...JSON.parse(producerManifest().toString("utf8")) }));
  manifestVersion.core_manifest_bytes_base64 = withSyntheticVersion.toString("base64");
  manifestVersion.core_manifest_size = withSyntheticVersion.length;
  manifestVersion.core_manifest_sha256 = sha256(withSyntheticVersion);
  assert.throws(() => validatePin(manifestVersion), /exactly bundles and wheel/);
  const wrongAssetVersion = fixturePin();
  wrongAssetVersion.core_version = "0.8.0";
  assert.throws(() => validatePin(wrongAssetVersion), /versioned release asset/);
  const alteredAssetPin = fixturePin();
  alteredAssetPin.core_assets[0].size += 1;
  assert.throws(() => validatePin(alteredAssetPin), /do not match the bundles/);
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

test("R4-b artifact pins may omit connector sidecars when the core manifest attestation is pinned", () => {
  const pin = fixturePin();
  pin.provenance.sidecars = [];
  assert.equal(validatePin(pin), pin);

  const noManifestSidecar = fixturePin();
  noManifestSidecar.core_manifest_sidecars = [];
  assert.throws(() => validatePin(noManifestSidecar), /no pinned core manifest attestation sidecars/);

  pin.core_manifest_sidecars = [];
  assert.throws(() => validatePin(pin), /no pinned core manifest attestation sidecars/);
});

test("SEA version and embedded core metadata must match the pin", () => {
  const pin = fixturePin();
  const version = { ok: true, version: pin.connector_version, target: pin.target, channel: pin.channel,
    connector_sha: pin.connector_sha, build_seq: pin.build_seq, format: "sea", sea: true };
  const metadata = { ok: true,
    connector: { version: pin.connector_version, sha: pin.connector_sha, channel: pin.channel, build_seq: pin.build_seq,
      target: pin.target, format: "sea", sea: true, link_min: pin.link_min, link_max: pin.link_max },
    embedded_core: { version: pin.core_version, manifest_sha256: pin.core_manifest_sha256,
      assets: pin.core_assets.map((asset) => ({ ...asset })), api: pin.core_api, link: pin.core_link },
    protocols: { metadata: pin.metadata_protocol, progress: pin.progress_protocol } };
  assert.equal(verifyMetadata(pin, version, metadata), true);
  for (const candidate of [
    { ...version, format: "esm" },
    { ...version, sea: false },
    { ...version, sea: undefined },
  ]) assert.throws(() => verifyMetadata(pin, candidate, metadata), /version output is not.*SEA/);
  for (const candidate of [
    { ...metadata, connector: { ...metadata.connector, format: "esm" } },
    { ...metadata, connector: { ...metadata.connector, sea: false } },
  ]) assert.throws(() => verifyMetadata(pin, version, candidate), /connector identity is not.*SEA/);
  metadata.embedded_core.manifest_sha256 = "0".repeat(64);
  assert.throws(() => verifyMetadata(pin, version, metadata), /embedded_core.manifest_sha256/);
  metadata.embedded_core.manifest_sha256 = pin.core_manifest_sha256;
  metadata.embedded_core.assets[0].sha256 = "0".repeat(64);
  assert.throws(() => verifyMetadata(pin, version, metadata), /embedded_core.assets/);
});


test("pins reject unsafe or colliding sidecar output names", () => {
  for (const name of [".", "..", "sidevoice", "connector-artifact.zip", "../manifest.json"]) {
    const pin = fixturePin(); pin.core_manifest_sidecars[0].name = name;
    assert.throws(() => validatePin(pin), /sidecar/);
  }
  const pin = fixturePin(); pin.provenance.sidecars[0].name = pin.core_manifest_sidecars[0].name;
  assert.throws(() => validatePin(pin), /duplicate/);
});
