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
DATA = Path.home() / ".sidevoice"
LABELS = ("dev.sidevoice.core", "dev.sidevoice.connector")
ALLOWED_STEPS = {
    "download", "verify", "stage", "service-start", "wait-calls", "wait-lock", "commit", "pairing", "rollback"
}
MAX_STDOUT = 1024 * 1024
MAX_LINE = 64 * 1024
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


def read_pin_and_verify_sea():
    require(sys.platform == "darwin" and platform.machine() == "arm64", "requires a native macOS arm64 runner")
    app = APP.resolve(strict=True)
    resources = app / "Contents/Resources/resources"
    pin = json.loads((resources / "connector-pin.json").read_text(encoding="utf-8"))
    require(pin.get("status") == "ready" and pin.get("target") == "macos-aarch64", "packaged connector pin is not ready for macOS arm64")
    sea = resources / "sidevoice"
    info = sea.stat()
    require(stat.S_ISREG(info.st_mode) and info.st_mode & stat.S_IXUSR, "packaged SEA is not an executable file")
    require(info.st_size == pin.get("executable_size"), "packaged SEA size does not match the reviewed pin")
    digest = hashlib.sha256()
    with sea.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    require(digest.hexdigest() == pin.get("executable_sha256"), "packaged SEA digest does not match the reviewed pin")
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
        require(isinstance(code, str) and 4 <= len(code) <= 32
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
