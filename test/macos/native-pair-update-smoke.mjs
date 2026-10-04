import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { chmod, lstat, mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, relative, resolve, sep } from "node:path";

const APP = resolve(process.env.APP || "src-tauri/target/packages/native-pair/Sidevoice.app");
const PROBE_APP = resolve(process.env.DOGFOOD_APP || "src-tauri/target/packages/native-pair-probe/Sidevoice.app");
const PREVIOUS_PIN = process.env.PREVIOUS_PAIR_PIN ? resolve(process.env.PREVIOUS_PAIR_PIN) : "";
const PREVIOUS_SEA = process.env.PREVIOUS_PAIR_SEA ? resolve(process.env.PREVIOUS_PAIR_SEA) : "";
const APP_CONFIG_DIR = resolve(process.env.HOME || "", "Library/Application Support/dev.sidevoice.desktop");
const MAX_OUTPUT = 2 * 1024 * 1024;
const MARKERS = {
  upgrade: "local-host-pair-update ok action=upgrade changed-pair=true reachable=true pairing-preserved=true",
  rollback: "local-host-pair-rollback ok error=install.rollback previous-pair=true reachable=true pairing-preserved=true",
};
const INTERRUPT = new AbortController();
const CHILDREN = new Set();

function trackChild(child) {
  CHILDREN.add(child);
  const forget = () => CHILDREN.delete(child);
  child.once("close", forget);
  child.once("error", forget);
}

function interrupt(signal) {
  if (INTERRUPT.signal.aborted) {
    for (const child of CHILDREN) { try { process.kill(-child.pid, "SIGKILL"); } catch {} }
    return;
  }
  INTERRUPT.abort(new Error(`smoke interrupted by ${signal}.`));
  const children = [...CHILDREN];
  for (const child of children) { try { process.kill(-child.pid, "SIGINT"); } catch {} }
  const timer = setTimeout(() => {
    for (const child of children) { if (CHILDREN.has(child)) { try { process.kill(-child.pid, "SIGKILL"); } catch {} } }
  }, 5_000);
  timer.unref();
}

process.on("SIGINT", () => interrupt("SIGINT"));
process.on("SIGTERM", () => interrupt("SIGTERM"));

function requireValue(ok, message) {
  if (!ok) throw new Error(message);
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function childEnv(extra = {}) {
  const env = {};
  for (const key of ["HOME", "PATH", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "USER", "LOGNAME", "SHELL"]) {
    if (process.env[key]) env[key] = process.env[key];
  }
  if (process.env.SIDEVOICE_DATA_DIR) env.SIDEVOICE_DATA_DIR = process.env.SIDEVOICE_DATA_DIR;
  return { ...env, ...extra };
}

function runChecked(file, args, action) {
  const result = spawnSync(file, args, { encoding: "utf8", timeout: 15_000, stdio: ["ignore", "pipe", "pipe"] });
  requireValue(!result.error && result.status === 0, `${action} failed.`);
  return `${result.stdout || ""}${result.stderr || ""}`;
}

function runJson(executable, args, { action, timeoutMs = 30_000 } = {}) {
  const env = childEnv();
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, args, { env, stdio: ["ignore", "pipe", "pipe"], detached: true });
    trackChild(child);
    const out = [];
    let bytes = 0;
    let timer;
    let killTimer;
    let requestedError;
    let settled = false;
    const stop = (error) => {
      if (requestedError) return;
      requestedError = error;
      try { process.kill(-child.pid, "SIGINT"); } catch {}
      killTimer = setTimeout(() => { try { process.kill(-child.pid, "SIGKILL"); } catch {} }, 60_000);
    };
    const settle = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      clearTimeout(killTimer);
      if (error) rejectPromise(error);
      else resolvePromise(value);
    };
    timer = setTimeout(() => stop(new Error(`${action} exceeded its deadline.`)), timeoutMs);
    child.stdout.on("data", (chunk) => {
      bytes += chunk.length;
      if (bytes > MAX_OUTPUT) return stop(new Error(`${action} exceeded bounded JSON output.`));
      out.push(chunk);
    });
    child.stderr.on("data", (chunk) => {
      bytes += chunk.length;
      if (bytes > MAX_OUTPUT) stop(new Error(`${action} exceeded bounded diagnostic output.`));
    });
    child.once("error", (error) => settle(new Error(`${action} could not start (${error.name}).`)));
    child.once("close", (status, signal) => {
      if (requestedError) return settle(requestedError);
      let result;
      try { result = JSON.parse(Buffer.concat(out).toString("utf8")); }
      catch { return settle(new Error(`${action} returned no valid JSON (status ${status ?? signal}).`)); }
      if (status !== 0 || result?.ok !== true) {
        const key = result?.error?.key;
        return settle(new Error(`${action} refused (${typeof key === "string" ? key : "unknown"}).`));
      }
      settle(null, result);
    });
  });
}

async function readPair(pinPath, seaPath, expectedSource) {
  const pinBytes = await readFile(pinPath);
  const pin = JSON.parse(pinBytes.toString("utf8"));
  requireValue(pin.schema === 2 && pin.status === "ready" && pin.target === "macos-aarch64"
    && pin.connector_sha === expectedSource && pin.native_pair?.runtime_kind === "rust-native-v1"
    && pin.native_pair?.core_kind === "rust-native-v1", "source-built pair pin is not the expected native pair.");
  const info = await lstat(seaPath);
  requireValue(info.isFile() && !info.isSymbolicLink() && (info.mode & 0o111), "source-built pair SEA is not executable.");
  const bytes = await readFile(seaPath);
  requireValue(bytes.length === pin.executable_size && sha256(bytes) === pin.executable_sha256,
    "source-built pair SEA differs from its exact source pin.");
  const version = JSON.parse(runChecked(seaPath, ["--version", "--json"], "source-built pair identity"));
  requireValue(version.sea === true && version.connector_sha === expectedSource
    && version.version === pin.connector_version && version.target === pin.target,
  "source-built pair runtime identity differs from its source pin.");
  return { pin, bytes, seaPath };
}

async function parseRelease(recordPath, expectedRoot) {
  const installed = JSON.parse(await readFile(recordPath, "utf8"));
  const root = await realpath(installed.releases);
  const fromTemp = relative(expectedRoot, root);
  requireValue(fromTemp !== ".." && !fromTemp.startsWith(`..${sep}`) && !fromTemp.startsWith(sep),
    "install record release root is outside the isolated test data directory.");
  const current = resolve(root, "current");
  const selectedDir = await realpath(current);
  const release = JSON.parse(await readFile(resolve(selectedDir, "release.json"), "utf8"));
  requireValue(release.id === selectedDir.split(sep).at(-1), "selected release metadata does not match its directory.");
  return { root, release, selectedDir };
}

function assertStatus(status, coreVersion, action) {
  requireValue(status?.ok === true && status.state === "running" && status.service === "launchd"
    && status.installed === true && status.reachable === true && status.core?.version === coreVersion
    && status.connector?.running === true, `${action} did not confirm a reachable native service.`);
}

async function verifyLaunchdClean(uid) {
  runChecked("/bin/launchctl", ["print", `gui/${uid}`], "macOS GUI launchd domain");
  const agents = resolve(process.env.HOME, "Library/LaunchAgents");
  for (const label of ["dev.sidevoice.core", "dev.sidevoice.connector"]) {
    try { await lstat(resolve(agents, `${label}.plist`)); throw new Error(`runner already has launchd service ${label}.`); }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    const probe = spawnSync("/bin/launchctl", ["print", `gui/${uid}/${label}`], { encoding: "utf8", timeout: 15_000 });
    requireValue(probe.status !== 0 && /could not find service/i.test(`${probe.stdout || ""}${probe.stderr || ""}`),
      `cannot confirm launchd service ${label} is absent.`);
  }
}

function startPackagedFlow(executable, kind, hooksDir) {
  const flowFlag = kind === "upgrade" ? "SIDEVOICE_DEBUG_LOCAL_HOST_PAIR_UPGRADE" : "SIDEVOICE_DEBUG_LOCAL_HOST_PAIR_ROLLBACK";
  const env = childEnv({ SIDEVOICE_DEBUG: "1", [flowFlag]: "1", ...(hooksDir ? { SIDEVOICE_TEST_HOOKS: hooksDir } : {}) });
  const child = spawn(executable, [], { env, stdio: ["ignore", "pipe", "pipe"], detached: true });
  trackChild(child);
  let total = 0;
  let pending = "";
  let outputTooLong = false;
  let timer;
  let killTimer;
  let settled = false;
  let outcome = null;
  let resolveResult;
  let rejectResult;
  const result = new Promise((resolvePromise, rejectPromise) => { resolveResult = resolvePromise; rejectResult = rejectPromise; });
  const terminate = () => {
    if (child.exitCode !== null || child.signalCode !== null) return;
    try { process.kill(-child.pid, "SIGTERM"); } catch {}
    killTimer = setTimeout(() => { try { process.kill(-child.pid, "SIGKILL"); } catch {} }, 5_000);
  };
  const requestFinish = (value) => {
    if (outcome) return;
    outcome = value;
    terminate();
  };
  const consume = (chunk) => {
    total += chunk.length;
    if (total > MAX_OUTPUT) return requestFinish({ error: new Error("packaged app exceeded bounded diagnostic output.") });
    const lines = `${pending}${chunk.toString("utf8")}`.split("\n");
    pending = lines.pop() || "";
    for (const line of lines) {
      if (outputTooLong) { outputTooLong = false; continue; }
      if (line.includes(MARKERS[kind])) {
        process.stdout.write(`packaged app bridge ${kind}: passed\n`);
        return requestFinish({ ok: true });
      }
      const prefix = kind === "upgrade" ? "local-host-pair-update error " : "local-host-pair-rollback error ";
      const index = line.indexOf(prefix);
      if (index >= 0) {
        const key = line.slice(index + prefix.length).match(/^[a-z0-9._-]{1,80}/)?.[0] || "unknown";
        return requestFinish({ error: new Error(`packaged app bridge ${kind} failed (${key}).`) });
      }
    }
    if (pending.length > 16_384) { pending = ""; outputTooLong = true; }
  };
  child.stdout.on("data", consume);
  child.stderr.on("data", consume);
  child.once("error", (error) => requestFinish({ error: new Error(`packaged app could not start (${error.name}).`) }));
  child.once("close", (status, signal) => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    clearTimeout(killTimer);
    if (outcome?.error) rejectResult(outcome.error);
    else if (outcome?.ok) resolveResult();
    else rejectResult(new Error(`packaged app exited before the ${kind} marker (status ${status ?? signal}).`));
  });
  timer = setTimeout(() => requestFinish({ error: new Error(`packaged app bridge ${kind} exceeded its deadline.`) }), 15 * 60_000);
  return { child, result, terminate };
}

async function delay(ms, signal) {
  if (signal.aborted) throw signal.reason;
  await new Promise((resolvePromise, rejectPromise) => {
    const timer = setTimeout(() => { signal.removeEventListener("abort", onAbort); resolvePromise(); }, ms);
    const onAbort = () => { clearTimeout(timer); signal.removeEventListener("abort", onAbort); rejectPromise(signal.reason); };
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

async function waitForFile(path, timeoutMs, message, signal = INTERRUPT.signal) {
  const deadline = Date.now() + timeoutMs;
  while (!signal.aborted && Date.now() < deadline) {
    try { const info = await lstat(path); if (info.isFile() && !info.isSymbolicLink()) return; }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    await delay(100, signal);
  }
  if (signal.aborted) throw signal.reason;
  throw new Error(message);
}

async function main() {
  requireValue(process.platform === "darwin" && process.arch === "arm64", "requires a hosted macOS arm64 runner.");
  requireValue(process.env.HOME && PREVIOUS_PIN && PREVIOUS_SEA, "hosted pair update inputs are unavailable.");
  const uid = process.getuid?.();
  requireValue(Number.isSafeInteger(uid) && uid > 0, "runner account has no valid GUI uid.");
  await verifyLaunchdClean(uid);
  try { requireValue((await readdir(APP_CONFIG_DIR)).length === 0, "runner app configuration is not fresh."); }
  catch (error) { if (error.code !== "ENOENT") throw error; }

  const previous = await readPair(PREVIOUS_PIN, PREVIOUS_SEA, "c3aa3468e66e265897fdf0fccf5ad55d6fcf060f");
  const currentPinPath = resolve(APP, "Contents/Resources/resources/connector-pin.json");
  const currentSeaPath = resolve(APP, "Contents/Resources/resources/sidevoice");
  const current = await readPair(currentPinPath, currentSeaPath, "435fd315e657a4f1372fc524f773387797195f38");
  const probePinBytes = await readFile(resolve(PROBE_APP, "Contents/Resources/resources/connector-pin.json"));
  const probeSeaBytes = await readFile(resolve(PROBE_APP, "Contents/Resources/resources/sidevoice"));
  requireValue(probePinBytes.equals(await readFile(currentPinPath)) && sha256(probeSeaBytes) === sha256(current.bytes),
    "the app page driver does not contain the production app's exact native pair.");
  requireValue(previous.pin.connector_version === current.pin.connector_version
    && previous.pin.core_version === current.pin.core_version
    && previous.pin.native_pair.pair_id !== current.pin.native_pair.pair_id,
  "the prior and current source-built pairs are not a same-version changed-pair scenario.");

  const temp = await mkdtemp(resolve(tmpdir(), "sidevoice-r4-pair-update-"));
  const dataDir = resolve(temp, "data");
  const hooksDir = resolve(temp, "hooks");
  await mkdir(hooksDir, { mode: 0o700 });
  process.env.SIDEVOICE_DATA_DIR = dataDir;
  let activeFlow = null;
  let installAttempted = false;
  let evidence = {
    schema: 1,
    kind: "r4-native-pair-packaged-update-rollback-smoke",
    previous_connector_sha: previous.pin.connector_sha,
    current_connector_sha: current.pin.connector_sha,
    previous_pair_id: previous.pin.native_pair.pair_id,
    current_pair_id: current.pin.native_pair.pair_id,
    changed_pair_upgrade: "pending",
    failed_update_rollback: "pending",
  };
  let failure;
  try {
    installAttempted = true;
    const initial = await runJson(previous.seaPath,
      ["install", "--no-agents", "--service", "--json", "--progress=jsonl"],
      { action: "prior pair install", timeoutMs: 15 * 60_000 });
    requireValue(initial.ok === true, "prior source-built pair install did not complete.");
    const beforeStatus = await runJson(current.seaPath, ["service", "status", "--json"], { action: "prior pair status" });
    assertStatus(beforeStatus, previous.pin.core_version, "prior pair install");
    const before = await parseRelease(resolve(dataDir, "install.json"), temp);
    requireValue(before.release.pair_id === previous.pin.native_pair.pair_id
      && before.release.runtime_kind === "rust-native-v1", "Connector did not select the genuine prior Rust pair.");

    activeFlow = startPackagedFlow(resolve(PROBE_APP, "Contents/MacOS/sidevoice-desktop"), "upgrade");
    await activeFlow.result;
    activeFlow = null;
    const updated = await parseRelease(resolve(dataDir, "install.json"), temp);
    requireValue(updated.release.pair_id === current.pin.native_pair.pair_id,
      "packaged app bridge did not select the current pinned pair.");
    const updatedStatus = await runJson(current.seaPath, ["service", "status", "--json"], { action: "updated pair status" });
    assertStatus(updatedStatus, current.pin.core_version, "changed-pair app update");
    evidence.changed_pair_upgrade = "packaged-bridge-upgrade-reachable";

    const removed = await runJson(current.seaPath, ["uninstall", "--harness", "codex", "--json"],
      { action: "reset before rollback acceptance", timeoutMs: 3 * 60_000 });
    requireValue(removed.state === "absent", "reset before rollback did not uninstall the selected pair.");
    installAttempted = false;
    await rm(APP_CONFIG_DIR, { recursive: true, force: true });
    installAttempted = true;
    const reinstalled = await runJson(previous.seaPath,
      ["install", "--no-agents", "--service", "--json", "--progress=jsonl"],
      { action: "prior pair reinstall", timeoutMs: 15 * 60_000 });
    requireValue(reinstalled.ok === true, "prior pair reinstall did not complete.");
    const priorAgain = await parseRelease(resolve(dataDir, "install.json"), temp);
    requireValue(priorAgain.release.pair_id === previous.pin.native_pair.pair_id,
      "prior source-built pair was not restored for the rollback trial.");
    const priorAgainStatus = await runJson(current.seaPath, ["service", "status", "--json"], { action: "rollback baseline status" });
    assertStatus(priorAgainStatus, previous.pin.core_version, "rollback baseline");

    await writeFile(resolve(hooksDir, "pause-install-after-quiesce"), "pause\n", { mode: 0o600 });
    activeFlow = startPackagedFlow(resolve(PROBE_APP, "Contents/MacOS/sidevoice-desktop"), "rollback", hooksDir);
    const paused = resolve(hooksDir, "paused-install-after-quiesce");
    await Promise.race([
      waitForFile(paused, 8 * 60_000, "Connector update did not reach the post-stage/pre-commit barrier."),
      activeFlow.result.then(() => { throw new Error("packaged rollback flow finished before its staged failure was armed."); }),
    ]);

    const releasesDir = resolve(priorAgain.root, "releases");
    const staged = (await readdir(releasesDir)).filter((name) => name.includes(".tmp-"));
    requireValue(staged.length === 1, "expected one fully staged update release before commit.");
    const stagedRelease = resolve(releasesDir, staged[0]);
    const stagedDir = await lstat(stagedRelease);
    requireValue(stagedDir.isDirectory() && !stagedDir.isSymbolicLink(), "staged release directory is not private and regular.");
    const stagedMetadata = JSON.parse(await readFile(resolve(stagedRelease, "release.json"), "utf8"));
    requireValue(stagedMetadata.pair_id === current.pin.native_pair.pair_id,
      "staged release does not carry the app's pinned current pair identity.");
    const stagedCore = resolve(stagedRelease, "core", "bin", "sidevoice-core-rust");
    const stagedInfo = await lstat(stagedCore);
    requireValue(stagedInfo.isFile() && !stagedInfo.isSymbolicLink() && (stagedInfo.mode & 0o111),
      "staged native Core entrypoint is not a regular executable.");
    await writeFile(stagedCore, "#!/bin/sh\nexit 0\n", { mode: 0o700 });
    await chmod(stagedCore, 0o700);
    await writeFile(resolve(hooksDir, "resume-install-after-quiesce"), "resume\n", { mode: 0o600 });
    await activeFlow.result;
    activeFlow = null;

    const recovered = await parseRelease(resolve(dataDir, "install.json"), temp);
    requireValue(recovered.release.pair_id === previous.pin.native_pair.pair_id,
      "failed update did not restore the prior verified Rust pair.");
    const recoveredStatus = await runJson(current.seaPath, ["service", "status", "--json"], { action: "rolled-back pair status" });
    assertStatus(recoveredStatus, previous.pin.core_version, "automatic Connector rollback");
    evidence.failed_update_rollback = "packaged-bridge-reported-rollback-prior-pair-reachable";
  } catch (error) {
    failure = error;
  } finally {
    await writeFile(resolve(hooksDir, "resume-install-after-quiesce"), "cleanup\n", { mode: 0o600 }).catch(() => {});
    activeFlow?.terminate();
    if (activeFlow) await activeFlow.result.catch(() => {});
    if (installAttempted) {
      try {
        const removed = await runJson(current.seaPath, ["uninstall", "--harness", "codex", "--json"],
          { action: "final pair cleanup", timeoutMs: 3 * 60_000 });
        requireValue(removed.state === "absent", "final pair cleanup did not confirm absence.");
        const status = await runJson(current.seaPath, ["service", "status", "--json"], { action: "post-cleanup status" });
        requireValue(status.state === "absent" && status.installed === false && status.reachable === false,
          "the native service remains after cleanup.");
        await verifyLaunchdClean(uid);
      } catch (cleanupError) { failure = failure || cleanupError; }
    }
    await rm(APP_CONFIG_DIR, { recursive: true, force: true });
    await rm(temp, { recursive: true, force: true });
  }
  if (failure) throw failure;
  requireValue(evidence.changed_pair_upgrade !== "pending" && evidence.failed_update_rollback !== "pending",
    "pair update acceptance did not finish both transaction paths.");
  if (process.env.SIDEVOICE_R4_UPDATE_SMOKE_EVIDENCE) {
    await mkdir(dirname(process.env.SIDEVOICE_R4_UPDATE_SMOKE_EVIDENCE), { recursive: true });
    await writeFile(process.env.SIDEVOICE_R4_UPDATE_SMOKE_EVIDENCE, `${JSON.stringify(evidence, null, 2)}\n`);
  }
  process.stdout.write("packaged app changed-pair update and automatic rollback passed\n");
}

main().catch((error) => {
  process.stderr.write(`Rust-native pair update smoke failed: ${error.message}\n`);
  process.exitCode = 1;
});
