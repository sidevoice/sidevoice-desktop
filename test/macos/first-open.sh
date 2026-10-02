#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -euo pipefail
APP="${APP:-src-tauri/target/packages/production/Sidevoice.app}"
# Asking for the probe page changes nothing in a release: the interface loads (checked below).
SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_PAGE=probe.html SIDEVOICE_DEBUG_ROOM_FLOW=1 "$APP/Contents/MacOS/sidevoice-desktop" > /tmp/app.log 2>&1 &
PID=$!
for i in $(seq 1 45); do grep -q "ready: true" /tmp/app.log && break; sleep 1; done
sleep 5
if ! kill -0 "$PID" 2>/dev/null; then echo "The app exited on its own:"; cat /tmp/app.log; exit 1; fi
kill "$PID"; cat /tmp/app.log
# The interface loaded from the app itself, and its controller came up: the bridge attached to the
# real web UI's store and actions, and reached the app over IPC.
grep -q "page tauri://localhost/voice/index.html" /tmp/app.log
if grep -Eq "probe.html|room-flow" /tmp/app.log; then echo "a release ran a CI probe"; exit 1; fi
grep -q "ready: true" /tmp/app.log
