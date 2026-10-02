#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -euo pipefail
APP="${APP:-src-tauri/target/packages/production/Sidevoice.app}"
CONF="$HOME/Library/Application Support/dev.sidevoice.desktop"
mkdir -p "$CONF"
printf '{"target":"http://127.0.0.1:8768","muteShortcut":""}' > "$CONF/settings.json"
python3 -u test/fixtures/fake-node.py 8768 > /tmp/node.log 2>&1 &
NODE=$!
SIDEVOICE_DEBUG=1 "$APP/Contents/MacOS/sidevoice-desktop" > /tmp/target.log 2>&1 &
PID=$!
for i in $(seq 1 45); do grep -q "ready: true" /tmp/target.log && break; sleep 1; done
sleep 5
kill "$PID" || true
kill "$NODE" || true  # the probe below starts its own on the same port
rm -rf "$CONF"
echo "--- app"; cat /tmp/target.log
echo "--- fake node (what the interface asked, cross-origin)"; sort /tmp/node.log | uniq -c
grep -q "ready: true" /tmp/target.log
# The interface asked its target from the app's own origin (cross-origin), with no pairing yet.
grep -q "origin=tauri://localhost" /tmp/node.log
