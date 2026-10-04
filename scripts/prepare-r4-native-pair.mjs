import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { lstat, readFile, readdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validatePin } from "./connector-pin.mjs";

const TARGETS = ["macos-aarch64", "linux-x86_64", "linux-aarch64"];
const ENTRYPOINT = "bin/sidevoice-core-rust";
const SHA256 = /^[0-9a-f]{64}$/;
const GIT_SHA = /^[0-9a-f]{40}$/;

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function exactKeys(value, keys) {
  return value && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).sort().join(",") === [...keys].sort().join(",");
}

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function commandJson(executable, args, { requireOk = true } = {}) {
  const text = execFileSync(executable, args, { encoding: "utf8", timeout: 20_000, maxBuffer: 1024 * 1024 });
  const value = JSON.parse(text);
  if (!value || typeof value !== "object" || Array.isArray(value) || (requireOk && value.ok !== true)) {
    throw new Error(`Pinned executable refused ${args.join(" ")}.`);
  }
  return value;
}

export function assertRustConnectorRuntimeIdentity(runtime, { target, sourceSha } = {}) {
  if (!exactKeys(runtime, ["kind", "target", "source_sha", "version"])
      || runtime.kind !== "rust-native-v1" || runtime.target !== target
      || runtime.source_sha !== sourceSha || typeof runtime.version !== "string" || runtime.version.length === 0) {
    throw new Error("Rust Connector runtime identity differs from the pinned source build.");
  }
  return runtime;
}

function gitSha(directory) {
  return execFileSync("git", ["-C", directory, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
}

async function readRegularFile(path, maxBytes) {
  const info = await lstat(path);
  if (!info.isFile() || info.isSymbolicLink() || info.size < 1 || info.size > maxBytes) {
    throw new Error(`Pinned source input is not a bounded regular file: ${path}`);
  }
  return readFile(path);
}

export async function prepareNativePairPin({ connectorRoot, coreRoot, coreInputs, rustConnector, sea, output,
  sourcePinPath = resolve("src-tauri/r4-native-pair-source-pin.json") } = {}) {
  for (const [name, value] of Object.entries({ connectorRoot, coreRoot, coreInputs, rustConnector, sea, output })) {
    if (!value) throw new Error(`Missing native pair build input: ${name}`);
  }
  const sourcePin = JSON.parse(await readFile(sourcePinPath, "utf8"));
  if (!exactKeys(sourcePin, ["schema", "connector", "core", "target"]) || sourcePin.schema !== 1
      || sourcePin.connector?.repository !== "sidevoice/sidevoice-connector"
      || sourcePin.core?.repository !== "sidevoice/sidevoice-core" || sourcePin.target !== "macos-aarch64"
      || !GIT_SHA.test(sourcePin.connector.source_sha || "") || !GIT_SHA.test(sourcePin.core.source_sha || "")
      || !SHA256.test(sourcePin.core.cargo_lock_sha256 || "")) {
    throw new Error("Desktop native pair source pin is malformed.");
  }
  const connectorSha = sourcePin.connector.source_sha;
  const coreSourceSha = sourcePin.core.source_sha;
  if (gitSha(resolve(connectorRoot)) !== connectorSha || gitSha(resolve(coreRoot)) !== coreSourceSha) {
    throw new Error("Checked-out Connector or Core source differs from Desktop's exact source pin.");
  }
  const connectorCorePin = JSON.parse(await readFile(resolve(connectorRoot, "packages/connector/rust-core-production-pin.json"), "utf8"));
  if (!exactKeys(connectorCorePin, ["schema", "repository", "source_sha", "cargo_lock_sha256"])
      || connectorCorePin.schema !== 1 || connectorCorePin.repository !== sourcePin.core.repository
      || connectorCorePin.source_sha !== coreSourceSha
      || connectorCorePin.cargo_lock_sha256 !== sourcePin.core.cargo_lock_sha256) {
    throw new Error("Connector's Rust Core pin differs from Desktop's source pair pin.");
  }
  const cargoLock = await readRegularFile(resolve(coreRoot, "Cargo.lock"), 50_000_000);
  if (sha256(cargoLock) !== sourcePin.core.cargo_lock_sha256) {
    throw new Error("Pinned Core Cargo.lock digest does not match Desktop's source pair pin.");
  }

  const manifestPath = resolve(coreInputs, "native-core-manifest.json");
  const manifestBytes = await readRegularFile(manifestPath, 4_000_000);
  const manifest = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(manifestBytes));
  if (!Buffer.from(`${canonicalJson(manifest)}\n`, "utf8").equals(manifestBytes)
      || !exactKeys(manifest, ["schema", "kind", "source_sha", "cargo_lock_sha256", "entrypoint", "bundles"])
      || manifest.schema !== 1 || manifest.kind !== "rust-native-v1" || manifest.source_sha !== coreSourceSha
      || manifest.cargo_lock_sha256 !== sourcePin.core.cargo_lock_sha256 || manifest.entrypoint !== ENTRYPOINT
      || !exactKeys(manifest.bundles, TARGETS)) {
    throw new Error("Core source inputs do not have the pinned canonical closed manifest.");
  }
  const expectedFiles = ["native-core-manifest.json"];
  for (const target of TARGETS) {
    const record = manifest.bundles[target];
    const name = `sidevoice-core-rust-${coreSourceSha}-${target}.tar.zst`;
    if (!exactKeys(record, ["name", "size", "sha256"]) || record.name !== name
        || !Number.isSafeInteger(record.size) || record.size < 1 || record.size > 250_000_000
        || !SHA256.test(record.sha256 || "")) {
      throw new Error(`Core closed manifest has an invalid ${target} archive record.`);
    }
    const bytes = await readRegularFile(resolve(coreInputs, name), 250_000_000);
    if (bytes.length !== record.size || sha256(bytes) !== record.sha256) {
      throw new Error(`Core ${target} archive differs from the closed manifest.`);
    }
    expectedFiles.push(name);
  }
  const actualFiles = (await readdir(coreInputs)).sort();
  if (actualFiles.join("\n") !== expectedFiles.sort().join("\n")) {
    throw new Error("Core source inputs contain files outside the exact closed archive set.");
  }

  const rustBytes = await readRegularFile(resolve(rustConnector), 100_000_000);
  const runtime = assertRustConnectorRuntimeIdentity(
    commandJson(resolve(rustConnector), ["runtime-identity", "--json"], { requireOk: false }),
    { target: sourcePin.target, sourceSha: connectorSha },
  );
  const seaBytes = await readRegularFile(resolve(sea), 512_000_000);
  const version = commandJson(resolve(sea), ["--version", "--json"]);
  const metadata = commandJson(resolve(sea), ["metadata", "--json"]);
  const connector = metadata.connector;
  const embeddedCore = metadata.embedded_core;
  if (version.format !== "sea" || version.sea !== true || version.connector_sha !== connectorSha
      || version.target !== sourcePin.target || version.version !== runtime.version
      || connector?.format !== "sea" || connector.sea !== true || connector.sha !== connectorSha
      || connector.target !== sourcePin.target || connector.version !== version.version
      || connector.channel !== version.channel || connector.build_seq !== version.build_seq
      || embeddedCore?.manifest_sha256 !== null || !Array.isArray(embeddedCore.assets) || embeddedCore.assets.length !== 0
      || metadata.protocols?.metadata !== "sidevoice-metadata-v1"
      || metadata.protocols?.progress !== "sidevoice-progress-jsonl-v1"
      || !Number.isSafeInteger(embeddedCore.api) || !Number.isSafeInteger(embeddedCore.link)
      || !Number.isSafeInteger(connector.link_min) || !Number.isSafeInteger(connector.link_max)) {
    throw new Error("Source-built SEA metadata does not match the R4 Rust-native pair contract.");
  }
  const archive = manifest.bundles[sourcePin.target];
  const coreBuild = `rust-native-v1-${sourcePin.target}-${coreSourceSha}-${archive.sha256}`;
  const pin = {
    schema: 2,
    status: "ready",
    target: sourcePin.target,
    connector_sha: connectorSha,
    connector_version: version.version,
    channel: version.channel,
    build_seq: version.build_seq,
    core_version: embeddedCore.version,
    executable_sha256: sha256(seaBytes),
    executable_size: seaBytes.length,
    metadata_protocol: metadata.protocols.metadata,
    progress_protocol: metadata.protocols.progress,
    core_api: embeddedCore.api,
    core_link: embeddedCore.link,
    link_min: connector.link_min,
    link_max: connector.link_max,
    native_pair: {
      runtime_kind: "rust-native-v1",
      runtime_build_sha: runtime.source_sha,
      runtime_sha256: sha256(rustBytes),
      runtime_size: rustBytes.length,
      runtime_target: runtime.target,
      core_kind: "rust-native-v1",
      core_source_sha: manifest.source_sha,
      core_cargo_lock_sha256: manifest.cargo_lock_sha256,
      core_manifest_sha256: sha256(manifestBytes),
      core_manifest_size: manifestBytes.length,
      core_manifest_bytes_base64: manifestBytes.toString("base64"),
      core_archive_sha256: archive.sha256,
      core_archive_size: archive.size,
      core_target: sourcePin.target,
      core_entrypoint: manifest.entrypoint,
      core_build: coreBuild,
      pair_id: `pair-v1:rust-native-v1:${sha256(rustBytes)}:core:${coreBuild}`,
    },
  };
  validatePin(pin);
  await writeFile(output, `${JSON.stringify(pin, null, 2)}\n`, { mode: 0o600 });
  return pin;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [connectorRoot, coreRoot, coreInputs, rustConnector, sea, output] = process.argv.slice(2);
  if (process.argv.length !== 8) throw new Error("Usage: prepare-r4-native-pair <connector> <core> <core-inputs> <rust-connector> <sea> <output>");
  const pin = await prepareNativePairPin({ connectorRoot, coreRoot, coreInputs, rustConnector, sea, output });
  process.stdout.write(`${JSON.stringify({ target: pin.target, connector_sha: pin.connector_sha,
    core_source_sha: pin.native_pair.core_source_sha, executable_sha256: pin.executable_sha256 })}\n`);
}
