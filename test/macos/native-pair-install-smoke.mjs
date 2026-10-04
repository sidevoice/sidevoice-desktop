import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { lstat, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";

const APP = resolve(process.env.APP || "src-tauri/target/packages/native-pair/Sidevoice.app");
const DOGFOOD_APP = resolve(process.env.DOGFOOD_APP || "src-tauri/target/packages/native-pair-probe/Sidevoice.app");
const APP_CONFIG_DIR = resolve(process.env.HOME || "", "Library/Application Support/dev.sidevoice.desktop");
const MAX_STDOUT = 1_048_576;
const MAX_STDERR_LINE = 65_536;
const MAX_APP_OUTPUT = 4_194_304;
const ALLOWED_PROGRESS = new Set(["stage", "verify", "service-start", "wait-calls", "wait-lock", "commit", "pairing", "rollback"]);
const DOGFOOD_SUCCESS = "local-host-dogfood ok local-cta=true progress=true remote-pairing=true reachable=true page-proxy=true update=noop";

function requireValue(ok, message) {
  if (!ok) throw new Error(message);
}

function safeError(result) {
  const key = result?.error?.key;
  return typeof key === "string" && /^[a-z0-9._-]{1,80}$/.test(key) ? key : "unknown";
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function runChecked(file, args, action) {
  const result = spawnSync(file, args, { encoding: "utf8", timeout: 15_000, stdio: ["ignore", "pipe", "pipe"] });
  requireValue(!result.error && result.status === 0, `${action} failed.`);
  return `${result.stdout || ""}${result.stderr || ""}`;
}

function childEnv() {
  const env = {};
  for (const key of ["HOME", "PATH", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "USER", "LOGNAME", "SHELL"]) {
    if (process.env[key]) env[key] = process.env[key];
  }
  if (process.env.SIDEVOICE_DATA_DIR) env.SIDEVOICE_DATA_DIR = process.env.SIDEVOICE_DATA_DIR;
  return env;
}

function runJson(executable, args, { action, timeoutMs = 30_000, progress = false, finalizeGraceMs = 90_000 } = {}) {
  const env = childEnv();
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(executable, args, { env, stdio: ["ignore", "pipe", "pipe"], detached: true });
    const out = [];
    let outBytes = 0;
    let errLine = Buffer.alloc(0);
    let errTooLong = false;
    let timer;
    let killTimer;
    let requestedError;
    let settled = false;
    const stop = (error) => {
      if (requestedError) return;
      requestedError = error;
      try { process.kill(-child.pid, "SIGINT"); } catch {}
      killTimer = setTimeout(() => {
        try { process.kill(-child.pid, "SIGKILL"); } catch {}
      }, finalizeGraceMs);
    };
    timer = setTimeout(() => stop(new Error(`${action} exceeded its deadline.`)), timeoutMs);
    const settle = (error, result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      clearTimeout(killTimer);
      if (error) rejectPromise(error);
      else resolvePromise(result);
    };
    child.once("error", (error) => settle(new Error(`${action} could not start (${error.name}).`)));
    child.stdout.on("data", (chunk) => {
      outBytes += chunk.length;
      if (outBytes > MAX_STDOUT) {
        stop(new Error(`${action} exceeded bounded JSON output.`));
        return;
      }
      out.push(chunk);
    });
    child.stderr.on("data", (chunk) => {
      if (!progress) return;
      for (const byte of chunk) {
        if (byte === 10) {
          if (!errTooLong && errLine.length) {
            try {
              const event = JSON.parse(errLine.toString("utf8"));
              if (event?.type === "progress" && ALLOWED_PROGRESS.has(event.step)) {
                process.stdout.write(`${action} progress: ${event.step}\n`);
              }
            } catch {}
          }
          errLine = Buffer.alloc(0);
          errTooLong = false;
        } else if (!errTooLong) {
          if (errLine.length >= MAX_STDERR_LINE) {
            errTooLong = true;
            errLine = Buffer.alloc(0);
          } else {
            errLine = Buffer.concat([errLine, Buffer.from([byte])]);
          }
        }
      }
    });
    child.once("close", (status, signal) => {
      if (requestedError) return settle(requestedError);
      let result;
      try { result = JSON.parse(Buffer.concat(out).toString("utf8")); }
      catch { return settle(new Error(`${action} returned no valid JSON (status ${status ?? signal}).`)); }
      if (status !== 0 || result?.ok !== true) return settle(new Error(`${action} refused (${safeError(result)}).`));
      settle(null, result);
    });
  });
}

async function verifyBundle(app) {
  const resourceDir = resolve(app, "Contents/Resources/resources");
  const pinPath = resolve(resourceDir, "connector-pin.json");
  const seaPath = resolve(resourceDir, "sidevoice");
  const executable = resolve(app, "Contents/MacOS/sidevoice-desktop");
  const pinBytes = await readFile(pinPath);
  const pin = JSON.parse(pinBytes.toString("utf8"));
  requireValue(pin.schema === 2 && pin.status === "ready" && pin.target === "macos-aarch64"
    && pin.native_pair?.runtime_kind === "rust-native-v1" && pin.native_pair?.core_kind === "rust-native-v1",
  "packaged application does not contain a ready Rust-native pair pin.");
  const seaInfo = await lstat(seaPath);
  requireValue(seaInfo.isFile() && !seaInfo.isSymbolicLink() && (seaInfo.mode & 0o111),
    "packaged source-built SEA is not an executable regular file.");
  const seaBytes = await readFile(seaPath);
  requireValue(seaBytes.length === pin.executable_size && sha256(seaBytes) === pin.executable_sha256,
    "packaged source-built SEA differs from the exact native pair pin.");
  const executableInfo = await lstat(executable);
  requireValue(executableInfo.isFile() && !executableInfo.isSymbolicLink(), "packaged Desktop executable is missing.");
  return { pin, pinBytes, seaBytes, seaPath, executable };
}

function runPackagedPage(appExecutable, timeoutMs = 40 * 60_000) {
  const env = {
    ...childEnv(),
    SIDEVOICE_DEBUG: "1",
    SIDEVOICE_DEBUG_LOCAL_HOST_DOGFOOD: "1",
  };
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(appExecutable, [], { env, stdio: ["ignore", "pipe", "pipe"], detached: true });
    let totalBytes = 0;
    let pending = "";
    let lineTooLong = false;
    let outcome = null;
    let timer;
    let terminateTimer;
    let settled = false;
    const terminate = () => {
      if (child.exitCode !== null || child.signalCode !== null) return;
      try { process.kill(-child.pid, "SIGTERM"); } catch {}
      terminateTimer = setTimeout(() => {
        try { process.kill(-child.pid, "SIGKILL"); } catch {}
      }, 5_000);
    };
    const fail = (error) => {
      if (outcome) return;
      outcome = { error };
      terminate();
    };
    const settle = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      clearTimeout(terminateTimer);
      if (error) rejectPromise(error);
      else resolvePromise();
    };
    const inspectLine = (line) => {
      if (outcome || lineTooLong) return;
      if (line.includes(DOGFOOD_SUCCESS)) {
        outcome = { ok: true };
        process.stdout.write("packaged page: local install progress, remote pairing, reachable Core proxy, and same-version update passed\n");
        terminate();
        return;
      }
      const match = /local-host-dogfood error ([a-z0-9-]{1,64})/.exec(line);
      if (match) fail(new Error(`packaged-page acceptance failed (${match[1]}).`));
    };
    const consume = (chunk) => {
      totalBytes += chunk.length;
      if (totalBytes > MAX_APP_OUTPUT) {
        fail(new Error("packaged Desktop app exceeded bounded diagnostic output."));
        return;
      }
      const text = chunk.toString("utf8");
      const lines = `${pending}${text}`.split("\n");
      pending = lines.pop() || "";
      for (const line of lines) {
        if (lineTooLong) lineTooLong = false;
        else inspectLine(line);
        if (outcome?.ok || outcome?.error) break;
      }
      if (!lineTooLong && pending.length > 16_384) {
        pending = "";
        lineTooLong = true;
      }
    };
    child.stdout.on("data", consume);
    child.stderr.on("data", consume);
    timer = setTimeout(() => fail(new Error("packaged-page acceptance exceeded its deadline.")), timeoutMs);
    child.once("error", (error) => settle(new Error(`packaged Desktop app could not start (${error.name}).`)));
    child.once("close", (status, signal) => {
      if (outcome?.error) return settle(outcome.error);
      if (outcome?.ok) return settle();
      settle(new Error(`packaged Desktop app exited before the page acceptance marker (status ${status ?? signal}).`));
    });
  });
}

function requireRunningStatus(status, coreVersion) {
  const core = status?.core && typeof status.core === "object" ? status.core : {};
  const connector = status?.connector && typeof status.connector === "object" ? status.connector : {};
  requireValue(status?.ok === true && status.state === "running" && status.service === "launchd"
    && status.installed === true && status.reachable === true && core.version === coreVersion
    && Number.isSafeInteger(core.pid) && core.pid > 1 && connector.running === true,
  "service status did not confirm the pinned native Core is running and reachable.");
  return core.pid;
}

async function preflight() {
  requireValue(process.platform === "darwin" && process.arch === "arm64", "requires a hosted macOS arm64 runner.");
  const uid = process.getuid?.();
  requireValue(Number.isSafeInteger(uid) && uid > 0, "runner account has no valid GUI uid.");
  runChecked("/bin/launchctl", ["print", `gui/${uid}`], "macOS GUI launchd domain");
  const agents = resolve(process.env.HOME, "Library/LaunchAgents");
  for (const label of ["dev.sidevoice.core", "dev.sidevoice.connector"]) {
    const plist = resolve(agents, `${label}.plist`);
    try { await lstat(plist); throw new Error(`runner already has launchd service ${label}.`); }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    const probe = spawnSync("/bin/launchctl", ["print", `gui/${uid}/${label}`], { encoding: "utf8", timeout: 15_000 });
    requireValue(probe.status !== 0 && /could not find service/i.test(`${probe.stdout || ""}${probe.stderr || ""}`),
      `cannot confirm launchd service ${label} is absent.`);
  }
  try {
    const entries = await readdir(APP_CONFIG_DIR);
    requireValue(entries.length === 0, "runner app configuration is not fresh.");
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  return { uid, agents };
}

async function verifyServicesAbsent(SEA, uid, agents) {
  const status = await runJson(SEA, ["service", "status", "--json"], { action: "post-uninstall status" });
  requireValue(status.state === "absent" && status.installed === false && status.reachable === false,
    "cleanup did not confirm that the service is absent.");
  for (const label of ["dev.sidevoice.core", "dev.sidevoice.connector"]) {
    const plist = resolve(agents, `${label}.plist`);
    try { await lstat(plist); throw new Error(`cleanup left launchd service ${label}.`); }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    const probe = spawnSync("/bin/launchctl", ["print", `gui/${uid}/${label}`], { encoding: "utf8", timeout: 15_000 });
    requireValue(probe.status !== 0 && /could not find service/i.test(`${probe.stdout || ""}${probe.stderr || ""}`),
      `cleanup could not confirm service ${label} is absent.`);
  }
  return status;
}

async function main() {
  requireValue(process.env.HOME, "runner HOME is unavailable.");
  const production = await verifyBundle(APP);
  const dogfood = await verifyBundle(DOGFOOD_APP);
  requireValue(production.pinBytes.equals(dogfood.pinBytes)
    && sha256(production.seaBytes) === sha256(dogfood.seaBytes),
  "the UI acceptance app does not contain the production app's exact native pair resources.");
  const pin = production.pin;
  const version = await runJson(production.seaPath, ["--version", "--json"], { action: "bundled SEA identity" });
  requireValue(version.connector_sha === pin.connector_sha && version.target === pin.target
    && version.version === pin.connector_version && version.sea === true,
  "packaged SEA runtime identity differs from its native pair pin.");

  const { uid, agents } = await preflight();
  const temp = await mkdtemp(resolve(tmpdir(), "sidevoice-r4-native-pair-"));
  process.env.SIDEVOICE_DATA_DIR = resolve(temp, "data");
  let installAttempted = false;
  let corePid = null;
  let failure;
  let smokeResult;
  try {
    installAttempted = true;
    await runPackagedPage(dogfood.executable);
    const status = await runJson(production.seaPath, ["service", "status", "--json"], { action: "app-installed service status" });
    corePid = requireRunningStatus(status, pin.core_version);
    process.stdout.write("service status: bundled Connector and pinned Core are running and reachable\n");
    smokeResult = {
      schema: 1,
      kind: "r4-native-pair-packaged-page-smoke",
      result: "passed",
      target: pin.target,
      connector_sha: pin.connector_sha,
      core_source_sha: pin.native_pair.core_source_sha,
      pair_id: pin.native_pair.pair_id,
      launch: "bundled-desktop-app-page",
      install: "local-cta-through-desktop-bridge-with-visible-progress",
      remote_pairing: "dialog-and-remote-install-command-available",
      reachable: "launchd-connector-and-pinned-core",
      page_proxy: "current-local-device-reached-core",
      same_version_update: "bridge-noop-kept-service-and-pairing",
      uninstall: "pending-cleanup",
    };
  } catch (error) {
    failure = error;
  } finally {
    if (installAttempted) {
      try {
        const result = await runJson(production.seaPath, ["uninstall", "--harness", "codex", "--json"], {
          action: "cleanup uninstall", timeoutMs: 3 * 60_000, finalizeGraceMs: 60_000,
        });
        requireValue(result.state === "absent", "cleanup uninstall did not report an absent service.");
        await verifyServicesAbsent(production.seaPath, uid, agents);
        if (corePid) {
          try { process.kill(corePid, 0); throw new Error("cleanup left the native Core process running."); }
          catch (error) { if (error.code !== "ESRCH") throw error; }
        }
        if (smokeResult) smokeResult.uninstall = "launchd-pair-absent";
        process.stdout.write("uninstall: launchd pair is absent after cleanup\n");
      } catch (cleanupError) {
        failure = failure || cleanupError;
      }
    }
    await rm(temp, { recursive: true, force: true });
    await rm(APP_CONFIG_DIR, { recursive: true, force: true });
  }
  if (failure) throw failure;
  if (process.env.SIDEVOICE_R4_SMOKE_EVIDENCE) {
    await mkdir(dirname(process.env.SIDEVOICE_R4_SMOKE_EVIDENCE), { recursive: true });
    await writeFile(process.env.SIDEVOICE_R4_SMOKE_EVIDENCE, `${JSON.stringify(smokeResult, null, 2)}\n`);
  }
  process.stdout.write("Rust-native pair packaged-page smoke passed\n");
}

main().catch((error) => {
  process.stderr.write(`Rust-native pair packaged-page smoke failed: ${error.message}\n`);
  process.exitCode = 1;
});
