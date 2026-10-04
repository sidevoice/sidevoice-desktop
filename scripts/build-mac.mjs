import { execFileSync } from "node:child_process";
import { chmod, copyFile, lstat, mkdtemp, readFile, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fetchConnector, sha256, validatePin, verifyArtifact } from "./connector-pin.mjs";
import { t } from "./build-i18n.mjs";

const mode = process.argv[2] ?? "production";
if (!["production", "fixture", "probe", "native-pair", "native-pair-probe"].includes(mode) || process.argv.length > 3) throw new Error(t("build.arguments"));
const nativePairMode = mode === "native-pair" || mode === "native-pair-probe";
const nativePairProbe = mode === "native-pair-probe";
const writeBuildEvidence = mode === "production" || mode === "native-pair";
if (process.platform !== "darwin" || process.arch !== "arm64") throw new Error(t("build.platform"));
const expectedNode = (await readFile(new URL("../.node-version", import.meta.url), "utf8")).trim();
if (process.version !== `v${expectedNode}`) throw new Error(t("build.node", { version: expectedNode }));
const rustc = execFileSync("rustc", ["--version"], { encoding: "utf8" }).trim();
const toolchain = /channel = "([^"]+)"/.exec(await readFile("rust-toolchain.toml", "utf8"))[1];
if (!rustc.startsWith(`rustc ${toolchain} `)) throw new Error(t("build.rust", { version: toolchain }));
const scratch = await mkdtemp(resolve(tmpdir(), "sidevoice-build-"));
// Reuse Cargo dependencies, but preserve each finished app separately before the next build.
// An inherited target directory must not make the reported package path ambiguous.
const targetDir = resolve("src-tauri/target");
const builtApp = resolve(targetDir, "aarch64-apple-darwin/release/bundle/macos/Sidevoice.app");
const app = resolve(targetDir, "packages", mode, "Sidevoice.app");
const env = { ...process.env, CARGO_TARGET_DIR: targetDir };
const sourcePinPath = resolve("src-tauri/connector-pin.json");
const sidevoiceResource = resolve("src-tauri/resources/sidevoice");
let restoreNativeInputs;
const trackedChangesBeforeBuild = execFileSync("git", ["status", "--porcelain", "--untracked-files=no"], { encoding: "utf8" }).trim();
const run = (file, args, extraEnv = {}) => execFileSync(file, args, { stdio: "inherit", env: { ...env, ...extraEnv } });
try {
  const expected = resolve(scratch, "fixture.json");
  if (writeBuildEvidence) {
    await rm("dist/build-evidence.json", { force: true });
  }
  if (mode === "production") {
    await fetchConnector();
  }
  if (nativePairMode) {
    const pinPath = process.env.SIDEVOICE_R4_NATIVE_PAIR_PIN;
    const seaPath = process.env.SIDEVOICE_R4_NATIVE_PAIR_SEA;
    if (!pinPath || !seaPath) throw new Error("native-pair build requires the hosted source-built pin and SEA paths.");
    const originalPin = await readFile(sourcePinPath);
    const pendingPin = JSON.parse(originalPin.toString("utf8"));
    if (pendingPin.status !== "pending") throw new Error("native-pair build only stages over the explicitly pending production pin.");
    const originalResource = await lstat(sidevoiceResource).then(async info => {
      if (!info.isFile() || info.isSymbolicLink()) throw new Error("existing bundled connector resource is not a regular file.");
      return { existed: true, bytes: await readFile(sidevoiceResource), mode: info.mode & 0o777 };
    }).catch(error => {
      if (error.code === "ENOENT") return { existed: false };
      throw error;
    });
    const nativePinBytes = await readFile(pinPath);
    const nativePin = validatePin(JSON.parse(nativePinBytes.toString("utf8")));
    const sourcePin = JSON.parse(await readFile("src-tauri/r4-native-pair-source-pin.json", "utf8"));
    if (sourcePin.schema !== 1 || sourcePin.target !== "macos-aarch64"
        || sourcePin.connector?.repository !== "sidevoice/sidevoice-connector"
        || sourcePin.core?.repository !== "sidevoice/sidevoice-core"
        || nativePin.connector_sha !== sourcePin.connector.source_sha
        || nativePin.native_pair?.core_source_sha !== sourcePin.core.source_sha
        || nativePin.native_pair?.core_cargo_lock_sha256 !== sourcePin.core.cargo_lock_sha256) {
      throw new Error("native-pair pin does not match the reviewed Desktop source pins.");
    }
    await verifyArtifact(seaPath, nativePin);
    restoreNativeInputs = async () => {
      await writeFile(sourcePinPath, originalPin);
      if (originalResource.existed) {
        await writeFile(sidevoiceResource, originalResource.bytes, { mode: originalResource.mode });
        await chmod(sidevoiceResource, originalResource.mode);
      } else {
        await rm(sidevoiceResource, { force: true });
      }
    };
    await writeFile(sourcePinPath, nativePinBytes);
    await mkdir(dirname(sidevoiceResource), { recursive: true });
    await copyFile(seaPath, sidevoiceResource);
    await chmod(sidevoiceResource, 0o755);
  }
  if (mode === "fixture") {
    const pin = JSON.parse(await readFile("src-tauri/connector-pin.json", "utf8"));
    if (pin.status !== "pending") throw new Error(t("build.fixture"));
    run(process.execPath, ["scripts/prepare-ci-connector.mjs", expected]);
  }
  const args = ["node_modules/@tauri-apps/cli/tauri.js", "build", "--target", "aarch64-apple-darwin", "--bundles", "app"];
  if (mode === "probe") args.push("--features", "probe");
  else {
    args.push("--config", "src-tauri/tauri.macos-aarch64.conf.json");
    if (nativePairProbe) args.push("--features", "probe");
  }
  args.push("--", "--locked");
  run(process.execPath, args);
  await mkdir(resolve(targetDir, "packages", mode), { recursive: true });
  await rm(app, { recursive: true, force: true });
  run("/usr/bin/ditto", [builtApp, app]);
  if (mode !== "probe") {
    run("bash", ["test/macos/check-package.sh"], {
      APP: app, CONNECTOR_FIXTURE: String(mode === "fixture"), CONNECTOR_FIXTURE_EXPECTED: expected,
      EXPECT_CI_PROBE: String(nativePairProbe),
    });
  }
  if (writeBuildEvidence) {
    const git = (...args) => execFileSync("git", args, { encoding: "utf8" }).trim();
    const pin = JSON.parse(await readFile(resolve(app, "Contents/Resources/resources/connector-pin.json"), "utf8"));
    const web = JSON.parse(await readFile("ui/voice/web-source.json", "utf8"));
    const evidence = {
      schema: 1, kind: mode === "native-pair" ? "r4-native-pair-candidate-package" : "production-package",
      desktop_sha: git("rev-parse", "HEAD"),
      tracked_changes: mode === "native-pair" ? trackedChangesBeforeBuild : git("status", "--porcelain", "--untracked-files=no"), web,
      node: process.version, rustc,
      github_cli: execFileSync("gh", ["--version"], { encoding: "utf8" }).trim(),
      app, app_executable_sha256: sha256(await readFile(resolve(app, "Contents/MacOS/sidevoice-desktop"))),
      connector_sha: pin.connector_sha, connector_sha256: pin.executable_sha256,
      core_manifest_sha256: pin.core_manifest_sha256 ?? pin.native_pair?.core_manifest_sha256,
      source_pair: pin.native_pair ? {
        runtime_kind: pin.native_pair.runtime_kind, runtime_sha256: pin.native_pair.runtime_sha256,
        core_kind: pin.native_pair.core_kind, core_source_sha: pin.native_pair.core_source_sha,
        core_archive_sha256: pin.native_pair.core_archive_sha256, pair_id: pin.native_pair.pair_id,
      } : null,
      provenance: pin.provenance ?? null,
      package_lock_sha256: sha256(await readFile("package-lock.json")),
      cargo_lock_sha256: sha256(await readFile("src-tauri/Cargo.lock")),
      // Packaging evidence does not assert the independent clean-account functional gate.
      clean_account_smoke: "not-run",
    };
    await mkdir("dist", { recursive: true });
    await writeFile("dist/build-evidence.json", `${JSON.stringify(evidence, null, 2)}\n`);
  }
  process.stdout.write(`${JSON.stringify({ mode, app })}\n`);
} finally {
  try {
    if (restoreNativeInputs) await restoreNativeInputs();
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}
