#!/usr/bin/env bash
# Run the R2/R3 host-scoped Agents probe in the CI-only app against a paired fake remote host.
set -euo pipefail
AGENTS_PROBE_APP="${AGENTS_PROBE_APP:-src-tauri/target/packages/probe/Sidevoice.app}"
AGENTS_NODE_LOG="${RUNNER_TEMP:-/tmp}/r2-r3-agents-node.log"
AGENTS_APP_LOG="${RUNNER_TEMP:-/tmp}/r2-r3-agents-app.log"
rm -f "$AGENTS_NODE_LOG" "$AGENTS_APP_LOG"
python3 -u test/fixtures/fake-node.py 8768 > "$AGENTS_NODE_LOG" 2>&1 &
AGENTS_NODE_PID=$!
AGENTS_APP_PID=""
cleanup() {
  if [ -n "$AGENTS_APP_PID" ]; then kill "$AGENTS_APP_PID" 2>/dev/null || true; fi
  kill "$AGENTS_NODE_PID" 2>/dev/null || true
  wait "$AGENTS_NODE_PID" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 50); do
  if curl -fsS --max-time 1 http://127.0.0.1:8768/api/rendezvous >/dev/null; then break; fi
  sleep 0.2
done
SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_AGENTS_WEB_FLOW=1 \
  "$AGENTS_PROBE_APP/Contents/MacOS/sidevoice-desktop" > "$AGENTS_APP_LOG" 2>&1 &
AGENTS_APP_PID=$!
for _ in $(seq 1 120); do
  if grep -Eq 'r2-r3-agents-flow (ok|error)' "$AGENTS_APP_LOG"; then break; fi
  sleep 1
done
grep -E 'page tauri://localhost/voice/index.html|r2-r3-agents-flow' "$AGENTS_APP_LOG" || true
grep -E 'request (OPTIONS|GET|POST) /api/(host/agents|presentation/languages)' "$AGENTS_NODE_LOG" || true
grep -q 'page tauri://localhost/voice/index.html' "$AGENTS_APP_LOG"
grep -Eq 'r2-r3-agents-flow ok settings=machines host=agents gear=marked get=authorized cursor-connect=authorized codex-replace=manual-visible codex-copy=(available|not-permitted|unavailable|unconfirmed) revoked=status' "$AGENTS_APP_LOG"
grep -Eq 'request GET /api/host/agents\?rescan=1 origin=tauri://localhost auth=yes' "$AGENTS_NODE_LOG"
grep -Eq 'request POST /api/host/agents/cursor/connect origin=tauri://localhost auth=yes' "$AGENTS_NODE_LOG"
if grep -Fq 'ci-agents-device-token' "$AGENTS_NODE_LOG" "$AGENTS_APP_LOG"; then
  echo 'probe logs exposed the fake pairing token'
  exit 1
fi
