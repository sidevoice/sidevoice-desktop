#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -euo pipefail
APP="${APP:-src-tauri/target/packages/probe/Sidevoice.app}"
# From nothing on disk, so the room downloads what it selects (the probe above downloaded into the same place).
rm -rf "$HOME/Library/Application Support/dev.sidevoice.desktop/engines"
# The machine the room is paired with (its identity proven; it stores the choices per machine).
python3 -u test/fixtures/fake-node.py 8768 > /tmp/room-node.log 2>&1 &
NODE=$!
# Real models; only a refused load (whisper-tiny) and a slow transcription (whisper-base on Core ML) injected.
SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_ROOM_FLOW=1 SIDEVOICE_DEBUG_REFUSE_LOAD=whisper-tiny \
  SIDEVOICE_DEBUG_SLOW_TRANSCRIBE=whisper-base/coreml:2500 \
  "$APP/Contents/MacOS/sidevoice-desktop" > /tmp/room.log 2>&1 &
PID=$!
for i in $(seq 1 600); do grep -Eq "room-flow (ok|error)" /tmp/room.log && break; sleep 1; done
kill "$PID" "$NODE" || true
cat /tmp/room.log
cat ui/voice/web-source.json
MODELS="$HOME/Library/Application Support/dev.sidevoice.desktop/engines/models/sherpa-onnx"
ls -la "$MODELS"
# The actual vendored room (ui/voice), not a test page: its settings actions, controller, selection, native
# worker and storage, over this app's bridge and engine (test/fixtures/room-flow.js).
grep -q "page tauri://localhost/voice/index.html" /tmp/room.log
if ! grep -q 'room-flow ok' /tmp/room.log; then
  echo "fake-node request paths (identity nonces redacted; auth values are never logged):"
  sed -E 's#(/api/device/identity)\?nonce=[^ ]+#\1?nonce=<redacted>#g' /tmp/room-node.log
fi
grep -q 'room-flow ok' /tmp/room.log
LINE=$(grep 'room-flow ok' /tmp/room.log)
# What the room asked the app, and the offers it resolved from it: the five native models, nothing of the page's.
echo "$LINE" | grep -q 'capabilities={"runs":"native","os":"macos","arch":"aarch64","has":\["cpu","coreml"'
echo "$LINE" | grep -Eq '"memory_mb":[1-9][0-9]{3,}'
echo "$LINE" | grep -Eq ' engines=(sherpa-onnx/[a-z]+,?)+ '
for model in whisper-tiny whisper-base whisper-small whisper-large-v3-turbo; do echo "$LINE" | grep -Eq " offers_stt=[^ ]*$model"; done
echo "$LINE" | grep -q ' offers_tts=kokoro-82m-v1.0 '
if echo "$LINE" | grep -Eq 'transformers-js|webgpu|wasm'; then echo "a page engine in the app"; exit 1; fi
echo "$LINE" | grep -Eq ' shown_stt=[^ ]*whisper-tiny'
echo "$LINE" | grep -Eq ' shown_tts=[^ ]*kokoro-82m-v1.0'
echo "$LINE" | grep -q ' where=app '
# 1. Consent with the size, then download → load → two passes → in effect, stored, loaded.
echo "$LINE" | grep -Eq ' consent_size=[1-9][0-9]{7,} stored_before=none chosen=whisper-base/auto chosen_resident=whisper-base@sherpa-onnx/cpu chosen_passes=2 '
# 2. Cancelled mid-download: nothing stored, nothing loaded, nothing installed, the download row cancelled.
echo "$LINE" | grep -Eq ' cancel_at=[1-9][0-9]*/[1-9][0-9]* cancel_download=cancelled cancel_stored=whisper-base/auto cancel_resident=whisper-base@sherpa-onnx/cpu cancel_installed=false '
if ls "$MODELS" | grep -q whisper-small; then echo "a cancelled download left files"; exit 1; fi
# 3. A failed load: its step and cause, the model in use still in use and alone in memory.
echo "$LINE" | grep -Eq ' fail_step=load fail_key=(runtime_failed|load_failed) fail_stored=whisper-base/auto fail_resident=whisper-base@sherpa-onnx/cpu '
# 4. Slow on Core ML: declined, nothing changes and its copy goes; accepted, it is stored and the old copy goes.
echo "$LINE" | grep -Eq ' slow_latency_ms=(2[0-9]{3}|[3-9][0-9]{3}) declined_stored=whisper-base/auto declined_resident=whisper-base@sherpa-onnx/cpu '
echo "$LINE" | grep -q ' accepted_stored=whisper-base/coreml accepted_resident=whisper-base@sherpa-onnx/coreml '
# 5. After a reload, with the machine reached: settings show the stored choice; the next selection starts from it.
echo "$LINE" | grep -q ' reload_reach=ok reload_restored=whisper-base/coreml reload_restored_pane=whisper-base/sherpa-onnx/coreml '
echo "$LINE" | grep -q ' reload_after_stored=whisper-base/auto reload_after_resident=whisper-base@sherpa-onnx/cpu'
grep -q 'request GET /api/device/identity' /tmp/room-node.log
