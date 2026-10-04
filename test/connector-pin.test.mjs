import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { sha256, validatePin, verifyMetadata } from "../scripts/connector-pin.mjs";
import { assertRustConnectorRuntimeIdentity } from "../scripts/prepare-r4-native-pair.mjs";

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
      artifact_name: "sidevoice-connector-macos-aarch64-r4b",
      sidecars: [{ name: "sidevoice-provenance.zip",
        url: "https://api.github.com/repos/sidevoice/sidevoice-connector/actions/artifacts/457/zip",
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

function nativePairPin() {
  const manifest = readFileSync(new URL("../test/fixtures/rust-native-core-manifest.json", import.meta.url));
  const coreSource = "e".repeat(40);
  const runtimeSha = "d".repeat(64);
  const archiveSha = "a".repeat(64);
  const coreBuild = `rust-native-v1-macos-aarch64-${coreSource}-${archiveSha}`;
  return {
    schema: 2, status: "ready", target: "macos-aarch64", connector_sha: "1".repeat(40),
    connector_version: "0.6.0", channel: "release", build_seq: 0, core_version: "0.1.0",
    executable_sha256: "2".repeat(64), executable_size: 4096,
    metadata_protocol: "sidevoice-metadata-v1", progress_protocol: "sidevoice-progress-jsonl-v1",
    core_api: 1, core_link: 1, link_min: 1, link_max: 1,
    native_pair: {
      runtime_kind: "rust-native-v1", runtime_build_sha: "1".repeat(40), runtime_sha256: runtimeSha,
      runtime_size: 2048, runtime_target: "macos-aarch64", core_kind: "rust-native-v1",
      core_source_sha: coreSource, core_cargo_lock_sha256: "f".repeat(64),
      core_manifest_sha256: sha256(manifest), core_manifest_size: manifest.length,
      core_manifest_bytes_base64: manifest.toString("base64"), core_archive_sha256: archiveSha,
      core_archive_size: 1, core_target: "macos-aarch64", core_entrypoint: "bin/sidevoice-core-rust",
      core_build: coreBuild, pair_id: `pair-v1:rust-native-v1:${runtimeSha}:core:${coreBuild}`,
    },
  };
}

test("Rust-native pin validates exact pair identity and closed Core archive schema without Python assets", () => {
  const pin = nativePairPin();
  assert.equal(validatePin(pin), pin);
  assert.equal(Object.hasOwn(pin, "core_assets"), false);

  for (const mutate of [
    (candidate) => { candidate.native_pair.pair_id = "pair-v1:javascript:fake"; },
    (candidate) => { candidate.native_pair.core_target = "linux-x86_64"; },
    (candidate) => {
      const bytes = Buffer.from(candidate.native_pair.core_manifest_bytes_base64, "base64").subarray(0, -1);
      candidate.native_pair.core_manifest_bytes_base64 = bytes.toString("base64");
      candidate.native_pair.core_manifest_size = bytes.length;
      candidate.native_pair.core_manifest_sha256 = sha256(bytes);
    },
    (candidate) => { candidate.core_assets = []; },
  ]) {
    const malformed = nativePairPin();
    mutate(malformed);
    assert.throws(() => validatePin(malformed), /native pair|native Core/);
  }
});

test("Rust runtime identity accepts the producer's strict JSON shape without an ok wrapper", () => {
  const identity = {
    kind: "rust-native-v1", target: "macos-aarch64",
    source_sha: "1".repeat(40), version: "0.6.0",
  };
  assert.equal(assertRustConnectorRuntimeIdentity(identity, {
    target: "macos-aarch64", sourceSha: "1".repeat(40),
  }), identity);
  for (const mutate of [
    (candidate) => { candidate.ok = true; },
    (candidate) => { candidate.target = "linux-x86_64"; },
    (candidate) => { candidate.source_sha = "2".repeat(40); },
    (candidate) => { candidate.version = ""; },
  ]) {
    const malformed = { ...identity };
    mutate(malformed);
    assert.throws(() => assertRustConnectorRuntimeIdentity(malformed, {
      target: "macos-aarch64", sourceSha: "1".repeat(40),
    }), /differs from the pinned source build/);
  }
});

test("hosted pair update baseline pins the prior Connector source to the same Core inputs", () => {
  const previous = JSON.parse(readFileSync(new URL("../test/fixtures/r4-native-pair-previous-source-pin.json", import.meta.url)));
  const current = JSON.parse(readFileSync(new URL("../src-tauri/r4-native-pair-source-pin.json", import.meta.url)));
  assert.deepEqual(Object.keys(previous).sort(), ["connector", "core", "schema", "target"]);
  assert.equal(previous.schema, 1);
  assert.equal(previous.target, current.target);
  assert.equal(previous.connector.repository, "sidevoice/sidevoice-connector");
  assert.equal(previous.connector.source_sha, "c3aa3468e66e265897fdf0fccf5ad55d6fcf060f");
  assert.deepEqual(previous.core, current.core, "only the exact Connector runtime source changes in this baseline pair");
});

test("Rust-native SEA metadata keeps Python manifest fields empty and verifies protocols", () => {
  const pin = nativePairPin();
  const version = { ok: true, version: pin.connector_version, target: pin.target, channel: pin.channel,
    connector_sha: pin.connector_sha, build_seq: pin.build_seq, format: "sea", sea: true };
  const metadata = { ok: true,
    connector: { version: pin.connector_version, sha: pin.connector_sha, channel: pin.channel,
      build_seq: pin.build_seq, target: pin.target, format: "sea", sea: true, link_min: 1, link_max: 1 },
    embedded_core: { version: pin.core_version, manifest_sha256: null, assets: [], api: pin.core_api, link: pin.core_link },
    protocols: { metadata: pin.metadata_protocol, progress: pin.progress_protocol } };
  assert.equal(verifyMetadata(pin, version, metadata), true);
  metadata.embedded_core.assets.push({ name: "python-core.whl" });
  assert.throws(() => verifyMetadata(pin, version, metadata), /distinct Rust-native Core schema/);
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

test("R4-b pins require the connector provenance ZIP as well as core manifest attestations", () => {
  for (const mutate of [
    (pin) => { delete pin.provenance.sidecars; },
    (pin) => { pin.provenance.sidecars = []; },
    (pin) => { pin.provenance.sidecars.push(pin.provenance.sidecars[0]); },
    (pin) => { pin.provenance.sidecars[0].name = "sidevoice.sigstore.json"; },
    (pin) => { pin.provenance.sidecars[0].url = pin.asset_url; },
    (pin) => { pin.provenance.sidecars[0].url = "https://github.com/sidevoice/sidevoice-connector/releases/download/nightly/provenance.zip"; },
  ]) {
    const pin = fixturePin(); mutate(pin);
    assert.throws(() => validatePin(pin), /requires one immutable/);
  }
  const pin = fixturePin();
  pin.core_manifest_sidecars = [];
  assert.throws(() => validatePin(pin), /no pinned core manifest attestation sidecars/);
  const branchPin = fixturePin();
  branchPin.provenance.workflow = ".github/workflows/r4-sea.yml@refs/heads/feature";
  assert.throws(() => validatePin(branchPin), /incomplete provenance/);
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
  const pin = fixturePin(); pin.core_manifest_sidecars[0].name = pin.provenance.sidecars[0].name;
  assert.throws(() => validatePin(pin), /duplicate/);
});
