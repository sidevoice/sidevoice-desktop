import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fetchConnector, sha256 } from "./connector-pin.mjs";
import { t } from "./build-i18n.mjs";

const mode = process.argv[2] ?? "production";
if (!["production", "fixture", "probe"].includes(mode) || process.argv.length > 3) throw new Error(t("build.arguments"));
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
const run = (file, args, extraEnv = {}) => execFileSync(file, args, { stdio: "inherit", env: { ...env, ...extraEnv } });
try {
  const expected = resolve(scratch, "fixture.json");
  if (mode === "production") {
    await rm("dist/build-evidence.json", { force: true });
    await fetchConnector();
  }
  if (mode === "fixture") {
    const pin = JSON.parse(await readFile("src-tauri/connector-pin.json", "utf8"));
    if (pin.status !== "pending") throw new Error(t("build.fixture"));
    run(process.execPath, ["scripts/prepare-ci-connector.mjs", expected]);
  }
  const args = ["node_modules/@tauri-apps/cli/tauri.js", "build", "--target", "aarch64-apple-darwin", "--bundles", "app"];
  if (mode === "probe") args.push("--features", "probe");
  else args.push("--config", "src-tauri/tauri.macos-aarch64.conf.json");
  args.push("--", "--locked");
  run(process.execPath, args);
  await mkdir(resolve(targetDir, "packages", mode), { recursive: true });
  await rm(app, { recursive: true, force: true });
  run("/usr/bin/ditto", [builtApp, app]);
  if (mode !== "probe") {
    run("bash", ["test/macos/check-package.sh"], {
      APP: app, CONNECTOR_FIXTURE: String(mode === "fixture"), CONNECTOR_FIXTURE_EXPECTED: expected,
    });
  }
  if (mode === "production") {
    const git = (...args) => execFileSync("git", args, { encoding: "utf8" }).trim();
    const pin = JSON.parse(await readFile(resolve(app, "Contents/Resources/resources/connector-pin.json"), "utf8"));
    const web = JSON.parse(await readFile("ui/voice/web-source.json", "utf8"));
    const evidence = {
      schema: 1, kind: "production-package", desktop_sha: git("rev-parse", "HEAD"),
      tracked_changes: git("status", "--porcelain", "--untracked-files=no"), web,
      node: process.version, rustc,
      app, app_executable_sha256: sha256(await readFile(resolve(app, "Contents/MacOS/sidevoice-desktop"))),
      connector_sha: pin.connector_sha, connector_sha256: pin.executable_sha256,
      core_manifest_sha256: pin.core_manifest_sha256, provenance: pin.provenance,
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
  await rm(scratch, { recursive: true, force: true });
}
