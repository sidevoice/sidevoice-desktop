import { createHash } from "node:crypto";
import { chmod, mkdir, mkdtemp, readFile, rename, rm, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { promisify } from "node:util";
import { execFile as execFileCallback, spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
import { Readable, Transform } from "node:stream";
import { pipeline } from "node:stream/promises";

const execFile = promisify(execFileCallback);
const SHA256 = /^[0-9a-f]{64}$/i;
const GIT_SHA = /^[0-9a-f]{40}$/i;
const MAX_ARTIFACT_BYTES = 512 * 1024 * 1024;

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function requirePin(condition, message) {
  if (!condition) throw new Error(`connector-pin.json ${message}`);
}

function githubAssetUrl(value) {
  return typeof value === "string" && (
    value.startsWith("https://github.com/sidevoice/sidevoice-connector/")
    || value.startsWith("https://api.github.com/repos/sidevoice/sidevoice-connector/")
    || value.startsWith("https://github.com/sidevoice/sidevoice-core/")
    || value.startsWith("https://api.github.com/repos/sidevoice/sidevoice-core/")
  );
}

function immutableAssetUrl(value, runId) {
  if (!githubAssetUrl(value)) return false;
  try {
    const url = new URL(value);
    const prefix = `/repos/sidevoice/sidevoice-connector/actions/runs/${runId}/artifacts/`;
    const artifact = url.pathname.startsWith(prefix) ? url.pathname.slice(prefix.length) : "";
    return url.protocol === "https:" && url.hostname === "api.github.com" && !url.search && !url.hash
      && artifact.endsWith("/zip") && /^[0-9]+$/.test(artifact.slice(0, -4));
  } catch { return false; }
}

export function validatePin(pin) {
  requirePin(pin && pin.schema === 1, "uses an unsupported schema.");
  requirePin(pin.status === "ready", "is pending R4-b's genuine signed macOS arm64 SEA and metadata/progress contract.");
  requirePin(pin.target === "macos-aarch64", "does not target macOS arm64.");
  requirePin(GIT_SHA.test(pin.connector_sha || ""), "has no valid connector git SHA.");
  requirePin(typeof pin.connector_version === "string" && pin.connector_version.length > 0, "has no connector version.");
  requirePin(["release", "nightly"].includes(pin.channel), "has no supported channel.");
  requirePin(Number.isSafeInteger(pin.build_seq) && pin.build_seq > 0, "has no build sequence.");
  requirePin(typeof pin.core_version === "string" && pin.core_version.length > 0, "has no core version.");
  requirePin(SHA256.test(pin.core_manifest_sha256 || ""), "has no valid embedded core manifest SHA-256.");
  requirePin(Number.isSafeInteger(pin.core_manifest_size) && pin.core_manifest_size > 0, "has no core manifest size.");
  let manifest;
  try { manifest = Buffer.from(pin.core_manifest_bytes_base64, "base64"); }
  catch { throw new Error("connector-pin.json has invalid base64 core manifest bytes."); }
  requirePin(manifest.toString("base64") === pin.core_manifest_bytes_base64,
    "core manifest bytes are not canonical base64.");
  requirePin(manifest.length === pin.core_manifest_size && sha256(manifest) === pin.core_manifest_sha256,
    "core manifest bytes do not match the pinned size and digest.");
  let parsedManifest;
  try { parsedManifest = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(manifest)); }
  catch { throw new Error("connector-pin.json core manifest bytes are not JSON."); }
  requirePin(parsedManifest?.version === pin.core_version, "core manifest version does not match the pin.");
  requirePin(Array.isArray(pin.core_assets) && pin.core_assets.length > 0, "has no pinned core assets.");
  for (const asset of pin.core_assets) {
    requirePin(asset && typeof asset.name === "string" && asset.name.length > 0 && typeof asset.url === "string"
      && asset.url.startsWith("https://") && SHA256.test(asset.sha256 || "")
      && Number.isSafeInteger(asset.size) && asset.size > 0, "has an invalid core asset pin.");
  }
  requirePin(SHA256.test(pin.executable_sha256 || ""), "has no valid SEA SHA-256.");
  requirePin(Number.isSafeInteger(pin.executable_size) && pin.executable_size > 0, "has no SEA byte size.");
  requirePin(immutableAssetUrl(pin.asset_url, pin.provenance?.run_id), "does not name an immutable connector artifact.");
  requirePin(pin.metadata_protocol === "sidevoice-metadata-v1" && pin.progress_protocol === "sidevoice-progress-jsonl-v1",
    "does not pin the R4-b metadata and progress protocols.");
  requirePin(Number.isSafeInteger(pin.core_api) && pin.core_api > 0 && Number.isSafeInteger(pin.core_link) && pin.core_link > 0,
    "has no embedded core API/link id.");
  requirePin(Number.isSafeInteger(pin.link_min) && Number.isSafeInteger(pin.link_max)
    && pin.link_min > 0 && pin.link_min <= pin.core_link && pin.core_link <= pin.link_max,
  "has an invalid connector/core link range.");
  const provenance = pin.provenance;
  requirePin(provenance?.repository === "sidevoice/sidevoice-connector"
    && typeof provenance.repository_id === "string" && provenance.repository_id.length > 0
    && typeof provenance.workflow === "string" && provenance.workflow.length > 0
    && Number.isSafeInteger(provenance.run_id) && provenance.run_id > 0
    && typeof provenance.artifact_name === "string" && provenance.artifact_name.length > 0,
  "has incomplete provenance.");
  const sidecars = [...(provenance.sidecars || []), ...(pin.core_manifest_sidecars || [])];
  requirePin(sidecars.length > 0, "has no pinned attestation sidecars.");
  for (const sidecar of sidecars) {
    requirePin(sidecar && typeof sidecar.name === "string" && /^[A-Za-z0-9._-]+$/.test(sidecar.name)
      && githubAssetUrl(sidecar.url)
      && SHA256.test(sidecar.sha256 || "") && Number.isSafeInteger(sidecar.size) && sidecar.size > 0,
    "has an invalid pinned attestation sidecar.");
  }
  return pin;
}

export function verifyMetadata(pin, version, metadata) {
  validatePin(pin);
  for (const [key, expected] of Object.entries({
    version: pin.connector_version,
    target: "macos-aarch64",
    channel: pin.channel,
    connector_sha: pin.connector_sha,
  })) requirePin(version?.[key] === expected, `version output ${key} does not match the pin.`);
  requirePin(version?.build_seq === pin.build_seq, "version output build_seq does not match the pin.");
  for (const [key, expected] of Object.entries({
    version: pin.connector_version,
    sha: pin.connector_sha,
    channel: pin.channel,
    build_seq: pin.build_seq,
    link_min: pin.link_min,
    link_max: pin.link_max,
  })) requirePin(metadata?.connector?.[key] === expected, `metadata connector.${key} does not match the pin.`);
  for (const [key, expected] of Object.entries({
    version: pin.core_version,
    manifest_sha256: pin.core_manifest_sha256,
    api: pin.core_api,
    link: pin.core_link,
  })) requirePin(metadata?.embedded_core?.[key] === expected, `embedded_core.${key} does not match the pin.`);
  requirePin(metadata?.protocols?.metadata === pin.metadata_protocol
    && metadata?.protocols?.progress === pin.progress_protocol, "metadata/progress protocols do not match the pin.");
  return true;
}

async function download(url, output, maxBytes) {
  const response = await fetch(url, { redirect: "follow", headers: { "user-agent": "sidevoice-desktop-build" } });
  if (!response.ok || !response.body) throw new Error(`Could not fetch pinned asset (${response.status}).`);
  const final = new URL(response.url);
  if (!(["github.com", "api.github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"].includes(final.hostname))) {
    throw new Error("Pinned asset redirected outside GitHub's public asset hosts.");
  }
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > maxBytes) throw new Error("Pinned asset exceeds the allowed size.");
  await pipeline(Readable.fromWeb(response.body), createWriteStream(output, { flags: "wx", mode: 0o600 }));
  const info = await stat(output);
  if (info.size === 0 || info.size > maxBytes) throw new Error("Downloaded asset size is invalid.");
}

async function commandJson(executable, args) {
  const { stdout } = await execFile(executable, args, { timeout: 20_000, maxBuffer: 1024 * 1024 });
  const result = JSON.parse(stdout);
  if (!result || result.ok !== true) throw new Error(`The pinned connector refused ${args.join(" ")}.`);
  return result;
}

async function verifySignature(executable) {
  await execFile("/usr/bin/codesign", ["--verify", "--strict", "--verbose=2", executable], { timeout: 20_000 });
  const { stderr, stdout } = await execFile("/usr/bin/codesign", ["-dv", "--verbose=2", executable], { timeout: 20_000 });
  if (!/Signature=adhoc/.test(`${stdout}\n${stderr}`)) throw new Error("The bundled connector has no valid ad-hoc macOS signature.");
}

export async function verifyArtifact(executable, pin, { platform = process.platform, arch = process.arch } = {}) {
  validatePin(pin);
  if (platform !== "darwin" || arch !== "arm64") throw new Error("Connector packaging requires a native macOS arm64 runner.");
  const info = await stat(executable);
  if (info.size !== pin.executable_size) throw new Error("Bundled connector byte size does not match connector-pin.json.");
  const bytes = await readFile(executable);
  if (sha256(bytes) !== pin.executable_sha256) throw new Error("Bundled connector SHA-256 does not match connector-pin.json.");
  const { stdout } = await execFile("/usr/bin/lipo", ["-archs", executable], { timeout: 20_000 });
  if (!stdout.split(/\s+/).includes("arm64")) throw new Error("Bundled connector is not an arm64 Mach-O executable.");
  await verifySignature(executable);
  const version = await commandJson(executable, ["--version", "--json"]);
  const metadata = await commandJson(executable, ["metadata", "--json"]);
  verifyMetadata(pin, version, metadata);
  return { size: info.size, sha256: pin.executable_sha256, version, metadata };
}

async function verifySidecars(pin, directory) {
  const records = [...pin.provenance.sidecars, ...pin.core_manifest_sidecars];
  for (const sidecar of records) {
    const path = resolve(directory, sidecar.name);
    await download(sidecar.url, path, Math.min(MAX_ARTIFACT_BYTES, sidecar.size + 1));
    const bytes = await readFile(path);
    if (bytes.length !== sidecar.size || sha256(bytes) !== sidecar.sha256) {
      throw new Error(`Pinned sidecar ${sidecar.name} failed its size or SHA-256 check.`);
    }
  }
}

async function extractPinnedExecutable(archive, output, expectedSize) {
  const { stdout: names } = await execFile("/usr/bin/unzip", ["-Z1", archive], { timeout: 20_000, maxBuffer: 1024 * 1024 });
  if (names.trim() !== "sidevoice") throw new Error("Pinned connector artifact must contain only the root sidevoice executable.");
  const child = spawn("/usr/bin/unzip", ["-p", archive, "sidevoice"], { stdio: ["ignore", "pipe", "pipe"] });
  const exited = new Promise((resolveStatus, reject) => {
    child.once("error", reject);
    child.once("close", resolveStatus);
  });
  const stderr = [];
  child.stderr.on("data", (chunk) => { if (Buffer.concat(stderr).length < 64 * 1024) stderr.push(chunk); });
  let extracted = 0;
  const exactSize = new Transform({
    transform(chunk, _encoding, callback) {
      extracted += chunk.length;
      callback(extracted > expectedSize ? new Error("Pinned connector archive expands beyond its expected executable size.") : null, chunk);
    },
    flush(callback) {
      callback(extracted === expectedSize ? null : new Error("Pinned connector archive has the wrong executable size."));
    },
  });
  try {
    await pipeline(child.stdout, exactSize, createWriteStream(output, { flags: "wx", mode: 0o600 }));
  } catch (error) {
    child.kill("SIGKILL");
    await exited.catch(() => {});
    throw error;
  }
  const status = await exited;
  if (status !== 0) throw new Error(`Could not extract the pinned connector executable: ${Buffer.concat(stderr).toString("utf8")}`);
}

export async function fetchConnector({
  pinPath = resolve("src-tauri/connector-pin.json"),
  outputPath = resolve("src-tauri/resources/sidevoice"),
  platform = process.platform,
  arch = process.arch,
} = {}) {
  const pin = validatePin(JSON.parse(await readFile(pinPath, "utf8")));
  if (platform !== "darwin" || arch !== "arm64") throw new Error("Connector packaging requires a native macOS arm64 runner.");
  await mkdir(dirname(outputPath), { recursive: true });
  // Keep the staging file beside the final resource so the final rename stays atomic on the output filesystem.
  const scratch = await mkdtemp(resolve(dirname(outputPath), ".sidevoice-connector-"));
  const staged = resolve(scratch, "sidevoice");
  const archive = resolve(scratch, "connector-artifact.zip");
  try {
    await verifySidecars(pin, scratch);
    await download(pin.asset_url, archive, Math.min(MAX_ARTIFACT_BYTES, pin.executable_size + 16 * 1024 * 1024));
    await extractPinnedExecutable(archive, staged, pin.executable_size);
    await chmod(staged, 0o755);
    await verifyArtifact(staged, pin, { platform, arch });
    await rename(staged, outputPath);
    return { outputPath, connector_sha: pin.connector_sha, executable_sha256: pin.executable_sha256,
      executable_size: pin.executable_size, core_manifest_sha256: pin.core_manifest_sha256, provenance: pin.provenance };
  } finally { await rm(scratch, { recursive: true, force: true }); }
}
