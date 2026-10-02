#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -euo pipefail
APP="${APP:-src-tauri/target/packages/probe/Sidevoice.app}"
python3 -u test/fixtures/fake-node.py 8768 > /tmp/probe-node.log 2>&1 &
NODE=$!
# A model stays in memory 5 s unused here (10 minutes in a release), so the probe sees D13 happen.
SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_PAGE=probe.html SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS=5 \
  "$APP/Contents/MacOS/sidevoice-desktop" > /tmp/probe.log 2>&1 &
PID=$!
for i in $(seq 1 240); do grep -q "probe-engine" /tmp/probe.log && break; sleep 1; done
kill "$PID" || true
cat /tmp/probe.log
echo "--- macOS $(sw_vers -productVersion)"
grep -q "page tauri://localhost/probe.html" /tmp/probe.log
# Pairing needs a secure context and WebCrypto ECDSA P-256 on the app's own origin.
grep -q "probe origin=tauri://localhost secure=true ecdsa=true" /tmp/probe.log
# A local node (http://127.0.0.1:8768) reached from the app's origin with a device token: preflight + CORS.
kill "$NODE" || true
grep -q 'probe-loopback 200 Bearer probe-token' /tmp/probe.log
grep -q 'request OPTIONS /api/presentation/echo origin=tauri://localhost' /tmp/probe-node.log
grep "sidevoice: media" /tmp/probe.log || echo "(WebKit did not ask the app about the microphone)"
# The native engine inside the signed app, through the bridge the interface uses (docs/BRIDGE.md):
# capabilities with the OS's real memory, downloads with progress, the builds on disk by model + engine,
# Kokoro says a sentence and Whisper hears it, and a page engine is refused rather than replaced.
grep -q 'probe-engine ok' /tmp/probe.log
grep -q 'probe-engine ok capabilities={"runs":"native","os":"macos","arch":"aarch64","has":\["cpu","coreml"' /tmp/probe.log
grep -Eq 'probe-engine ok .*"memory_mb":[1-9][0-9]{3,}' /tmp/probe.log
grep -q 'probe-engine ok .* installed=kokoro-82m-v1.0@sherpa-onnx,whisper-tiny@sherpa-onnx ' /tmp/probe.log
grep -Eq 'probe-engine ok .* progress_calls=[1-9]' /tmp/probe.log
grep -q 'probe-engine ok .* refused=yes ' /tmp/probe.log
grep -iq 'probe-engine ok.*prueba' /tmp/probe.log
# Load → transcribe → unload (sidevoice/sidevoice-core#21 phase 3): loaded once, one model in memory each for every
# language (Spanish, then English), freed on unload; the OS's memory and what is available of it.
grep -q 'probe-engine ok .* before_load=kokoro-82m-v1.0@sherpa-onnx/cpu load_ms=' /tmp/probe.log
grep -q 'probe-engine ok .* load_ms=\([0-9]*\) load_again_ms=\1 ' /tmp/probe.log
grep -q 'probe-engine ok .* loaded=kokoro-82m-v1.0@sherpa-onnx/cpu,whisper-tiny@sherpa-onnx/cpu since_ok=true same_models=true unloaded=kokoro-82m-v1.0@sherpa-onnx/cpu ' /tmp/probe.log
grep -Eq 'probe-engine ok .* memory=\{"total_mb":[1-9][0-9]{3,},"available_mb":[1-9][0-9]*\}' /tmp/probe.log
grep -Eiq 'probe-engine ok .* text_en="[^"]*(test|voice)' /tmp/probe.log
# D13: after the call, unused for the idle time, both unloaded by the app; loaded again as the next call connects.
grep -q 'sidevoice: engine unloaded idle whisper-tiny@sherpa-onnx/cpu' /tmp/probe.log
grep -q 'sidevoice: engine unloaded idle kokoro-82m-v1.0@sherpa-onnx/cpu' /tmp/probe.log
grep -q 'sidevoice: engine preload whisper-tiny@sherpa-onnx/cpu load_ms=' /tmp/probe.log
grep -q 'probe-engine ok .* preloaded=kokoro-82m-v1.0@sherpa-onnx/cpu,whisper-tiny@sherpa-onnx/cpu ' /tmp/probe.log
# Cancel: a download stopped mid-way rejects with install_cancelled; progress events carry the job, the build
# and the speed; nothing of it is left on disk.
grep -q 'probe-engine ok .* cancel=true/install_cancelled/false cancel_gone=true progress_event=bytes_per_s+done+engine+job+model+total:true ' /tmp/probe.log
grep -Eq 'probe-engine ok .* speed=[1-9][0-9]* ' /tmp/probe.log
MODELS="$HOME/Library/Application Support/dev.sidevoice.desktop/engines/models/sherpa-onnx"
ls -la "$MODELS"
if ls "$MODELS" | grep -q whisper-small; then echo "a cancelled download left files"; exit 1; fi
# Headset buttons: a (simulated) call made the app the Now Playing app, and it let go after.
grep -q 'probe-headset ok' /tmp/probe.log
grep -q 'headset apply active=true in_call=true mic=true' /tmp/probe.log
grep 'headset apply' /tmp/probe.log | tail -1 | grep -q 'active=false in_call=false .* gesture_on=false'
grep 'headset apply' /tmp/probe.log
# The app's own mute is not mistaken for an AirPods gesture.
if grep -q 'headset airpods' /tmp/probe.log; then echo 'echo recorded as a gesture'; exit 1; fi
