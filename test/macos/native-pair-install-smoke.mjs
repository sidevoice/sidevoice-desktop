import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { lstat, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";

const APP = resolve(process.env.APP || "src-tauri/target/packages/native-pair/Sidevoice.app");
const RESOURCE_DIR = resolve(APP, "Contents/Resources/resources");
const SEA = resolve(RESOURCE_DIR, "sidevoice");
const PIN_PATH = resolve(RESOURCE_DIR, "connector-pin.json");
const ALLOWED_PROGRESS = new Set(["stage", "verify", "service-start", "wait-calls", "wait-lock", "commit", "pairing", "rollback"]);
const MAX_STDOUT = 1_048_576;
const MAX_STDERR_LINE = 65_536;

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

function runJson(executable, args, { action, timeoutMs = 30_000, progress = false, finalizeGraceMs = 90_000 } = {}) {
  const env = {};
  for (const key of ["HOME", "PATH", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "USER", "LOGNAME", "SHELL"]) {
    if (process.env[key]) env[key] = process.env[key];
  }
  if (process.env.SIDEVOICE_DATA_DIR) env.SIDEVOICE_DATA_DIR = process.env.SIDEVOICE_DATA_DIR;
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
  return { uid, agents };
}

async function main() {
  const pinBytes = await readFile(PIN_PATH);
  const pin = JSON.parse(pinBytes.toString("utf8"));
  requireValue(pin.schema === 2 && pin.status === "ready" && pin.target === "macos-aarch64"
    && pin.native_pair?.runtime_kind === "rust-native-v1" && pin.native_pair?.core_kind === "rust-native-v1",
  "packaged application does not contain a ready Rust-native pair pin.");
  const seaInfo = await lstat(SEA);
  requireValue(seaInfo.isFile() && !seaInfo.isSymbolicLink() && (seaInfo.mode & 0o111),
    "packaged source-built SEA is not an executable regular file.");
  const seaBytes = await readFile(SEA);
  requireValue(seaBytes.length === pin.executable_size && sha256(seaBytes) === pin.executable_sha256,
    "packaged source-built SEA differs from the exact native pair pin.");
  const version = await runJson(SEA, ["--version", "--json"], { action: "bundled SEA identity" });
  requireValue(version.connector_sha === pin.connector_sha && version.target === pin.target
    && version.version === pin.connector_version && version.sea === true,
  "packaged SEA runtime identity differs from its native pair pin.");

  const { uid, agents } = await preflight();
  const temp = await mkdtemp(resolve(tmpdir(), "sidevoice-r4-native-pair-"));
  process.env.SIDEVOICE_DATA_DIR = resolve(temp, "data");
  let installAttempted = false;
  let corePid = null;
  let failure;
  try {
    installAttempted = true;
    const install = await runJson(SEA, ["install", "--no-agents", "--service", "--json", "--progress=jsonl"], {
      action: "bundled service install", timeoutMs: 35 * 60_000, progress: true, finalizeGraceMs: 5 * 60_000,
    });
    requireValue(install.action === "install" && install.state === "running" && install.service === "launchd",
      "bundled install did not report a running launchd service.");
    let status = await runJson(SEA, ["service", "status", "--json"], { action: "installed service status" });
    corePid = requireRunningStatus(status, pin.core_version);
    process.stdout.write("bundled install: native Core is running and reachable under launchd\n");

    const pairing = await runJson(SEA, ["pair-device", "--json"], { action: "local device pairing code" });
    const code = pairing.code;
    requireValue(typeof code === "string" && /^SV1\.[A-Za-z0-9_-]+$/.test(code) && code.length <= 8192
      && Number.isSafeInteger(pairing.expires_in) && pairing.expires_in > 0
      && typeof pairing.reach === "string" && pairing.reach.length > 0,
    "bundled SEA did not produce a valid local-only device pairing code.");
    process.stdout.write("pair-device: local one-time code created without logging it\n");

    const repeated = await runJson(SEA, ["install", "--no-agents", "--service", "--json", "--progress=jsonl"], {
      action: "same-version install", timeoutMs: 10 * 60_000, progress: true, finalizeGraceMs: 2 * 60_000,
    });
    requireValue(repeated.action === "noop" && repeated.state === "running",
      "same-version install was not a safe running no-op.");
    status = await runJson(SEA, ["service", "status", "--json"], { action: "post-no-op service status" });
    corePid = requireRunningStatus(status, pin.core_version);
    process.stdout.write("same-version install: reachable service remained selected\n");
  } catch (error) {
    failure = error;
  } finally {
    if (installAttempted) {
      try {
        const result = await runJson(SEA, ["uninstall", "--harness", "codex", "--json"], {
          action: "cleanup uninstall", timeoutMs: 3 * 60_000, finalizeGraceMs: 60_000,
        });
        requireValue(result.state === "absent", "cleanup uninstall did not report an absent service.");
        const status = await runJson(SEA, ["service", "status", "--json"], { action: "post-uninstall status" });
        requireValue(status.state === "absent" && status.installed === false && status.reachable === false,
          "cleanup did not confirm that the service is absent.");
        for (const label of ["dev.sidevoice.core", "dev.sidevoice.connector"]) {
          const plist = resolve(agents, `${label}.plist`);
          try { await lstat(plist); throw new Error(`cleanup left launchd service ${label}.`); }
          catch (error) { if (error.code !== "ENOENT") throw error; }
          const probe = spawnSync("/bin/launchctl", ["print", `gui/${uid}/${label}`], { encoding: "utf8", timeout: 15_000 });
          requireValue(probe.status !== 0 && /could not find service/i.test(`${probe.stdout || ""}${probe.stderr || ""}`),
            `cleanup could not confirm launchd service ${label} is absent.`);
        }
        if (corePid) {
          try { process.kill(corePid, 0); throw new Error("cleanup left the native Core process running."); }
          catch (error) { if (error.code !== "ESRCH") throw error; }
        }
        process.stdout.write("uninstall: launchd pair is absent after cleanup\n");
      } catch (cleanupError) {
        failure = failure || cleanupError;
      }
    }
    await rm(temp, { recursive: true, force: true });
  }
  if (failure) throw failure;
  if (process.env.SIDEVOICE_R4_SMOKE_EVIDENCE) {
    await mkdir(dirname(process.env.SIDEVOICE_R4_SMOKE_EVIDENCE), { recursive: true });
    await writeFile(process.env.SIDEVOICE_R4_SMOKE_EVIDENCE, `${JSON.stringify({
      schema: 1,
      kind: "r4-native-pair-install-smoke",
      result: "passed",
      target: pin.target,
      connector_sha: pin.connector_sha,
      core_source_sha: pin.native_pair.core_source_sha,
      pair_id: pin.native_pair.pair_id,
      install: "launchd-running-reachable",
      local_pairing_code: "issued-unlogged",
      repeat_install: "same-version-noop",
      uninstall: "absent",
    }, null, 2)}\n`);
  }
  process.stdout.write("Rust-native pair app-bundle smoke passed\n");
}

main().catch((error) => {
  process.stderr.write(`Rust-native pair app-bundle smoke failed: ${error.message}\n`);
  process.exitCode = 1;
});
