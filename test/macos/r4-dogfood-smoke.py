#!/usr/bin/env python3
"""Exercise the production-bundled R4 connector on a disposable macOS runner account."""

import hashlib
import json
import os
import platform
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path


APP = Path(os.environ.get("APP", "src-tauri/target/packages/production/Sidevoice.app"))
PROBE_APP = Path(os.environ.get("PROBE_APP", "src-tauri/target/packages/probe/Sidevoice.app"))
DATA = Path.home() / ".sidevoice"
APP_CONFIG = Path.home() / "Library/Application Support/dev.sidevoice.desktop"
PAIRING_FILE = APP_CONFIG / "local-host.json"
LABELS = ("dev.sidevoice.core", "dev.sidevoice.connector")
ALLOWED_STEPS = {
    "download", "verify", "stage", "service-start", "wait-calls", "wait-lock", "commit", "pairing", "rollback"
}
MAX_STDOUT = 1024 * 1024
MAX_LINE = 64 * 1024
MAX_APP_OUTPUT = 2 * 1024 * 1024
APP_LINE_LIMIT = 16 * 1024
PROCESS = None
INTERRUPTED = None
IN_CLEANUP = False


class SmokeFailure(RuntimeError):
    pass


def say(message):
    print(message, flush=True)


def on_signal(signum, _frame):
    global INTERRUPTED
    if IN_CLEANUP:
        return
    INTERRUPTED = signal.Signals(signum).name


signal.signal(signal.SIGINT, on_signal)
signal.signal(signal.SIGTERM, on_signal)


def require(ok, message):
    if not ok:
        raise SmokeFailure(message)


def child_environment():
    # Explicit allowlist: no Actions, GitHub, Sidevoice credentials, room values, or caller secrets.
    env = {key: os.environ[key] for key in (
        "HOME", "PATH", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "USER", "LOGNAME", "SHELL"
    ) if key in os.environ}
    env["SIDEVOICE_DATA_DIR"] = str(DATA)
    return env


def sha256_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_pin_and_verify_sea():
    require(sys.platform == "darwin" and platform.machine() == "arm64", "requires a native macOS arm64 runner")
    app = APP.resolve(strict=True)
    probe_app = PROBE_APP.resolve(strict=True)
    require((probe_app / "Contents/MacOS/sidevoice-desktop").is_file(), "probe app executable is missing")
    resources = app / "Contents/Resources/resources"
    pin_path = resources / "connector-pin.json"
    pin_bytes = pin_path.read_bytes()
    require(pin_bytes == Path("src-tauri/connector-pin.json").read_bytes(),
            "production app pin does not match the source pin compiled into the probe app")
    pin = json.loads(pin_bytes)
    require(pin.get("status") == "ready" and pin.get("target") == "macos-aarch64", "packaged connector pin is not ready for macOS arm64")
    sea = resources / "sidevoice"
    info = sea.stat()
    require(stat.S_ISREG(info.st_mode) and info.st_mode & stat.S_IXUSR, "packaged SEA is not an executable file")
    require(info.st_size == pin.get("executable_size"), "packaged SEA size does not match the reviewed pin")
    sea_sha256 = sha256_file(sea)
    require(sea_sha256 == pin.get("executable_sha256"), "packaged SEA digest does not match the reviewed pin")
    version = run_json(sea, ["--version", "--json"], timeout=20, action="SEA version")
    for key, expected in {
        "format": "sea", "sea": True, "version": pin.get("connector_version"),
        "target": "macos-aarch64", "channel": pin.get("channel"),
        "connector_sha": pin.get("connector_sha"), "build_seq": pin.get("build_seq"),
    }.items():
        require(version.get(key) == expected, f"SEA version identity does not match pin field {key}")
    return sea, pin


def run_json(sea, args, timeout, action, require_ok=True, progress=False,
             require_progress=False, finalize_grace=60):
    global PROCESS
    if INTERRUPTED and not IN_CLEANUP:
        raise SmokeFailure(f"interrupted by {INTERRUPTED}")
    try:
        process = subprocess.Popen(
            [str(sea), *args], env=child_environment(), stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
        )
    except OSError as exc:
        raise SmokeFailure(f"could not start {action} ({type(exc).__name__})") from None
    PROCESS = process
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ, "stdout")
    selector.register(process.stderr, selectors.EVENT_READ, "stderr")
    stdout = bytearray()
    stderr_line = bytearray()
    progress_steps = []
    deadline = time.monotonic() + timeout
    kill_deadline = None
    forced_kill = False
    last_heartbeat = time.monotonic()
    stderr_overlong = False

    try:
        while selector.get_map() or process.poll() is None:
            now = time.monotonic()
            if INTERRUPTED and kill_deadline is None and not forced_kill:
                say(f"{action}: interruption received; requesting connector cancellation")
                kill_deadline = now + finalize_grace
                try:
                    os.killpg(process.pid, signal.SIGINT)
                except ProcessLookupError:
                    pass
            elif kill_deadline is None and not forced_kill and now >= deadline:
                say(f"{action}: command deadline reached; requesting connector cancellation")
                kill_deadline = now + finalize_grace
                try:
                    os.killpg(process.pid, signal.SIGINT)
                except ProcessLookupError:
                    pass
            elif kill_deadline is not None and now >= kill_deadline and process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                kill_deadline = None
                forced_kill = True
            if now - last_heartbeat >= 30 and process.poll() is None:
                say(f"{action}: still running")
                last_heartbeat = now

            for key, _ in selector.select(timeout=0.5):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                if key.data == "stdout":
                    require(len(stdout) + len(chunk) <= MAX_STDOUT, f"{action} exceeded the bounded JSON output size")
                    stdout.extend(chunk)
                elif progress:
                    for byte in chunk:
                        if byte == 10:
                            if not stderr_overlong and stderr_line:
                                try:
                                    event = json.loads(stderr_line)
                                except (UnicodeDecodeError, json.JSONDecodeError):
                                    event = None
                                if isinstance(event, dict) and event.get("type") == "progress":
                                    step = event.get("step")
                                    if isinstance(step, str) and step in ALLOWED_STEPS:
                                        if not progress_steps or progress_steps[-1] != step:
                                            progress_steps.append(step)
                                            say(f"install progress: {step}")
                            stderr_line.clear()
                            stderr_overlong = False
                        elif not stderr_overlong:
                            if len(stderr_line) >= MAX_LINE:
                                stderr_line.clear()
                                stderr_overlong = True
                            else:
                                stderr_line.append(byte)
        return_code = process.wait()
    finally:
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGINT)
                process.wait(timeout=finalize_grace)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
        selector.close()
        PROCESS = None

    if INTERRUPTED and not IN_CLEANUP:
        raise SmokeFailure(f"interrupted by {INTERRUPTED}")
    try:
        result = json.loads(stdout)
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise SmokeFailure(f"{action} exited with status {return_code} without valid JSON") from None
    require(isinstance(result, dict), f"{action} returned an unexpected JSON value")
    if return_code != 0:
        failure = result.get("failure")
        failure_key = failure.get("key") if isinstance(failure, dict) else None
        if not isinstance(failure_key, str) or not re.fullmatch(r"[a-z0-9._-]{1,80}", failure_key):
            failure_key = None
        detail = f"; underlying {failure_key}" if failure_key else ""
        raise SmokeFailure(f"{safe_refusal(action, result)}{detail}; exit status {return_code}")
    if require_ok:
        require(result.get("ok") is True, safe_refusal(action, result))
    if require_progress:
        # The release transaction opens its staging directory before the core runtime is downloaded.
        required_steps = ["stage", "download", "verify", "service-start", "pairing"]
        cursor = 0
        for step in progress_steps:
            if cursor < len(required_steps) and step == required_steps[cursor]:
                cursor += 1
        require(cursor == len(required_steps), "install omitted or reordered required progress phases")
    return result


def safe_refusal(action, result):
    key = result.get("error", {}).get("key") if isinstance(result.get("error"), dict) else None
    if isinstance(key, str) and re.fullmatch(r"[a-z0-9._-]{1,80}", key):
        return f"{action} was refused ({key})"
    return f"{action} was refused"


def launchctl(args, timeout=15):
    try:
        result = subprocess.run(["/bin/launchctl", *args], stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=timeout, check=False)
        return result.returncode, (result.stdout + result.stderr).decode("utf-8", "replace")
    except (OSError, subprocess.TimeoutExpired):
        return None, ""


def launchd_state(uid, label):
    return launchctl(["print", f"gui/{uid}/{label}"])


def require_launchd_label_absent(uid, label, context):
    code, output = launchd_state(uid, label)
    missing = re.search(r"could not find service", output, re.IGNORECASE)
    require(code is not None and code != 0 and missing is not None,
            f"could not confirm launchd service {label} is absent during {context}")


def preflight():
    require(not DATA.exists() and not DATA.is_symlink(), "runner account already has a Sidevoice data directory")
    require(not APP_CONFIG.exists() and not APP_CONFIG.is_symlink(),
            "runner account already has Sidevoice app configuration")
    agents = Path.home() / "Library/LaunchAgents"
    for label in LABELS:
        plist = agents / f"{label}.plist"
        require(not plist.exists() and not plist.is_symlink(), f"runner account already has launch agent {label}")
    uid = os.getuid()
    gui_code, _ = launchctl(["print", f"gui/{uid}"])
    require(gui_code == 0,
            "macOS GUI launchd domain is unavailable; use a disposable arm64 runner with an active GUI login")
    for label in LABELS:
        require_launchd_label_absent(uid, label, "preflight")
    return uid, agents


def stop_app(process, action):
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            raise SmokeFailure(f"could not stop {action}") from None


def run_app_until(app, env, markers, timeout, action, error_marker=None):
    """Launch a packaged app, consume bounded output without printing it, and require explicit readiness markers."""
    try:
        process = subprocess.Popen(
            [str(app / "Contents/MacOS/sidevoice-desktop")], env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True,
        )
    except OSError as exc:
        raise SmokeFailure(f"could not start {action} ({type(exc).__name__})") from None

    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ)
    selector.register(process.stderr, selectors.EVENT_READ)
    lines = {process.stdout: bytearray(), process.stderr: bytearray()}
    seen = set()
    total = 0
    deadline = time.monotonic() + timeout
    ready_at = None
    try:
        while time.monotonic() < deadline:
            if INTERRUPTED:
                raise SmokeFailure(f"interrupted by {INTERRUPTED}")
            if process.poll() is not None and not selector.get_map():
                break
            for key, _ in selector.select(timeout=0.25):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                total += len(chunk)
                require(total <= MAX_APP_OUTPUT, f"{action} exceeded bounded diagnostic output")
                line = lines[key.fileobj]
                for byte in chunk:
                    if byte == 10:
                        value = bytes(line)
                        line.clear()
                        if error_marker and error_marker in value:
                            match = re.search(rb"local-host-dogfood error ([a-z0-9-]{1,64})", value)
                            key_name = match.group(1).decode("ascii") if match else "unknown"
                            raise SmokeFailure(f"{action} reported {key_name}")
                        for marker in markers:
                            if marker in value:
                                seen.add(marker)
                    elif len(line) < APP_LINE_LIMIT:
                        line.append(byte)
                    else:
                        line.clear()
            if len(seen) == len(markers):
                require(process.poll() is None, f"{action} exited after reporting readiness")
                ready_at = ready_at or time.monotonic()
                if time.monotonic() - ready_at >= 5:
                    return
            if process.poll() is not None and not selector.get_map():
                break
        if time.monotonic() >= deadline:
            raise SmokeFailure(f"{action} did not become ready before its deadline")
        raise SmokeFailure(f"{action} exited before reporting readiness (status {process.returncode})")
    finally:
        selector.close()
        stop_app(process, action)


def launch_production_app():
    env = child_environment()
    env["SIDEVOICE_DEBUG"] = "1"
    markers = {
        b"page tauri://localhost/voice/index.html",
        b"ready: true",
        b"local-host paired ",
        b"local-host state running",
    }
    run_app_until(APP.resolve(strict=True), env, markers, timeout=120, action="production app")

    require(PAIRING_FILE.is_file() and not PAIRING_FILE.is_symlink(),
            "production app did not persist its local-host pairing")
    try:
        mode = stat.S_IMODE(PAIRING_FILE.stat().st_mode)
        pairing = json.loads(PAIRING_FILE.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        raise SmokeFailure("production app pairing record is unreadable") from None
    require(mode == 0o600 and all(isinstance(pairing.get(key), str) and pairing[key]
                                  for key in ("fp", "public_key", "device_id", "token", "host", "paired_at"))
            and len(pairing["token"]) == 43,
            "production app did not persist a private, complete pairing record")
    say("production app: bundled page became ready and paired the local host")


def verify_projected_host():
    env = child_environment()
    env["SIDEVOICE_DEBUG"] = "1"
    env["SIDEVOICE_DEBUG_LOCAL_HOST_DOGFOOD"] = "1"
    run_app_until(
        PROBE_APP.resolve(strict=True), env, {b"local-host-dogfood ok "},
        timeout=180, action="genuine-core bridge probe", error_marker=b"local-host-dogfood error",
    )
    say("bundled page: projected pairing reached the genuine core; same-version bridge update was a no-op")


def require_running_status(status, core_version):
    core = status.get("core") if isinstance(status.get("core"), dict) else {}
    connector = status.get("connector") if isinstance(status.get("connector"), dict) else {}
    require(status.get("ok") is True and status.get("state") == "running"
            and status.get("service") == "launchd" and status.get("installed") is True
            and status.get("reachable") is True and core.get("version") == core_version
            and isinstance(core.get("pid"), int) and core["pid"] > 1
            and connector.get("running") is True, "service status did not confirm the pinned core is running and reachable")
    return core["pid"]


def process_is_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def uninstall_and_verify(sea, uid, agents, core_pid):
    global IN_CLEANUP
    IN_CLEANUP = True
    if PROCESS is not None and PROCESS.poll() is None:
        try:
            os.killpg(PROCESS.pid, signal.SIGINT)
        except ProcessLookupError:
            pass
    result = run_json(sea, ["uninstall", "--harness", "codex", "--json"],
                      timeout=180, action="uninstall")
    require(result.get("state") == "absent", "uninstall did not report absent state")
    status = run_json(sea, ["service", "status", "--json"], timeout=30, action="post-uninstall status")
    connector = status.get("connector") if isinstance(status.get("connector"), dict) else {}
    require(status.get("state") == "absent" and status.get("installed") is False
            and status.get("reachable") is False and connector.get("running") is False,
            "post-uninstall status did not confirm the service is absent")
    deadline = time.monotonic() + 30
    while core_pid and process_is_alive(core_pid) and time.monotonic() < deadline:
        time.sleep(0.5)
    require(not core_pid or not process_is_alive(core_pid), "core process remained after uninstall")
    require(all(not (agents / f"{label}.plist").exists()
                and not (agents / f"{label}.plist").is_symlink() for label in LABELS),
            "launch agent plist remained after uninstall")
    for label in LABELS:
        require_launchd_label_absent(uid, label, "uninstall")
    require(not DATA.is_symlink(), "Sidevoice data path became a symlink")
    if DATA.exists():
        shutil.rmtree(DATA)
    require(not DATA.exists(), "could not remove the fresh Sidevoice data directory")
    require(not APP_CONFIG.is_symlink(), "app configuration path became a symlink")
    if APP_CONFIG.exists():
        shutil.rmtree(APP_CONFIG)
    require(not APP_CONFIG.exists(), "could not remove the fresh Sidevoice app configuration")
    say("uninstall: service, launch agents, core process, and fresh data directory are absent")


def main():
    sea, pin = read_pin_and_verify_sea()
    uid, agents = preflight()
    installed_pid = None
    install_attempted = False
    try:
        install_attempted = True
        installed = run_json(
            sea, ["install", "--no-agents", "--service", "--json", "--progress=jsonl"],
            timeout=50 * 60, action="install", progress=True, require_progress=True, finalize_grace=8 * 60,
        )
        require(installed.get("action") == "install" and installed.get("state") == "running"
                and installed.get("service") == "launchd", "install did not report a running launchd installation")
        status = run_json(sea, ["service", "status", "--json"], timeout=30, action="service status")
        installed_pid = require_running_status(status, pin["core_version"])
        say("install: pinned core is running and reachable under launchd")

        pairing = run_json(sea, ["pair-device", "--json"], timeout=30, action="pair-device")
        code = pairing.pop("code", None)
        # Core's SV1 code carries the identity fingerprint, address and one-time secret; it is not a short PIN.
        require(isinstance(code, str) and 24 <= len(code) <= 8192
                and code.startswith("SV1.") and re.fullmatch(r"[A-Za-z0-9_-]+", code[4:])
                and isinstance(pairing.get("expires_in"), int) and pairing["expires_in"] > 0
                and isinstance(pairing.get("reach"), str) and pairing["reach"],
                "pair-device did not return a valid local pairing code")
        del code, pairing
        say("pair-device: received a valid, unlogged local code")

        repeated = run_json(
            sea, ["install", "--no-agents", "--service", "--json", "--progress=jsonl"],
            timeout=10 * 60, action="same-version install", progress=True, finalize_grace=2 * 60,
        )
        require(repeated.get("action") == "noop" and repeated.get("state") == "running",
                "same-version install was not a running no-op")
        status = run_json(sea, ["service", "status", "--json"], timeout=30, action="post-noop status")
        installed_pid = require_running_status(status, pin["core_version"])
        say("same-version install: safe no-op preserved the reachable service")

        launch_production_app()
        verify_projected_host()
        status = run_json(sea, ["service", "status", "--json"], timeout=30, action="post-app status")
        installed_pid = require_running_status(status, pin["core_version"])
        say("post-app status: genuine core remains running and reachable")
    finally:
        if install_attempted:
            uninstall_and_verify(sea, uid, agents, installed_pid)
    say("R4 clean-account dogfood smoke passed")


if __name__ == "__main__":
    try:
        main()
    except (SmokeFailure, OSError, subprocess.SubprocessError) as exc:
        say(f"R4 clean-account dogfood smoke failed: {exc}")
        sys.exit(1)
