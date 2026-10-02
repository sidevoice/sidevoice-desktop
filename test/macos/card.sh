#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -euo pipefail
APP="${APP:-src-tauri/target/packages/probe/Sidevoice.app}"
mkdir -p card-shots
swiftc -O -o /tmp/card-ci test/fixtures/card-ci.swift
# The editor as an app of its own, so LaunchServices opens and activates it as it would the person's.
mkdir -p /tmp/Editor.app/Contents/MacOS
swiftc -O -o /tmp/Editor.app/Contents/MacOS/Editor test/fixtures/fullscreen-editor.swift
printf '%s' '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleExecutable</key><string>Editor</string><key>CFBundleIdentifier</key><string>dev.sidevoice.ci-editor</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleName</key><string>Editor</string></dict></plist>' > /tmp/Editor.app/Contents/Info.plist
/tmp/card-ci trusted
trap 'echo "--- app"; grep -E "call-controls|probe-card" /tmp/card.log || true; echo "--- editor"; cat /tmp/editor.log || true' EXIT
# Every check runs, and the step fails at the end with each that did not hold.
FAILED=""
fail() { echo "::error::$*"; FAILED="$FAILED"$'\n'"$*"; }
# wait_for <log> <pattern> [after line]: the pattern appears in the log, after that line if given.
wait_for() {
  for i in $(seq 1 60); do tail -n +"$((${3:-0} + 1))" "$1" | grep -q -- "$2" && return 0; sleep 1; done
  return 1
}
expect() { wait_for "$@" || fail "never saw in $1: $2"; }
line() { wc -l < "$1" | tr -d ' '; }
# The card's window as the window server has it on screen (global points): it must be in front of the editor.
card() {
  /tmp/card-ci windows "$PID" "$EDITOR" | tee /tmp/windows.txt || true
  grep -q "card onscreen=true above_editor=true editor_onscreen=true" /tmp/windows.txt \
    || fail "the card is not on screen in front of the editor: $(head -1 /tmp/windows.txt)"
  if grep -q " x=" /tmp/windows.txt; then
    read -r X Y W H < <(sed -E 's/.* x=(-?[0-9]+) y=(-?[0-9]+) w=([0-9]+) h=([0-9]+).*/\1 \2 \3 \4/' /tmp/windows.txt)
  fi
}
editor_active() {
  tail -1 < <(grep "editor active=" /tmp/editor.log) | grep -q "active=true" || fail "the editor is not the active app ($1)"
}

# A call walked through the card's states (test/fixtures/card-probe.html); this page answers its buttons.
# Each line with the time, to order it against the editor's and the input's.
SIDEVOICE_DEBUG=1 SIDEVOICE_DEBUG_PAGE=card-probe.html "$APP/Contents/MacOS/sidevoice-desktop" \
  > >(perl -MTime::HiRes=time -ne '$|=1; printf "%.2f %s", time, $_' > /tmp/card.log) 2>&1 &
PID=$!
wait_for /tmp/card.log "probe-card phase=talking" || { fail "the probe never started"; exit 1; }
# The person's editor, full screen in its own Space: the room's window loses focus and the card shows over it,
# a non-activating panel at the status level, on every Space and over full-screen apps, never the key window.
: > /tmp/editor.log
open --stdout /tmp/editor.log --stderr /tmp/editor.log /tmp/Editor.app
wait_for /tmp/editor.log "editor ready pid=" || { fail "the editor never started"; exit 1; }
EDITOR=$(sed -nE 's/.*editor ready pid=([0-9]+).*/\1/p' /tmp/editor.log | head -1)
expect /tmp/editor.log "editor fullscreen=true key=true"
wait_for /tmp/card.log "call-controls shown" || { fail "the card never showed"; exit 1; }
expect /tmp/card.log "call-controls panel "
grep "call-controls panel " /tmp/card.log | tail -1 \
  | grep -q "nonactivating=true level=25 all_spaces=true fullscreen_auxiliary=true key=false visible=true .* app_active=false" \
  || fail "the panel is not what #4 asks: $(grep 'call-controls panel ' /tmp/card.log | tail -1)"
expect /tmp/card.log "call-controls watching clicks elsewhere: true"
expect /tmp/card.log "call-controls prevents activation: true"
sleep 3
card
[ -n "${X:-}" ] || { fail "the card's window was never found"; exit 1; }
screencapture -x card-shots/1-over-fullscreen-you-talking.png
# The card's page may not capture: it asked for the microphone (probe build) and was refused.
expect /tmp/card.log 'call-controls refused {"capture":"[A-Za-z-]*","command":"probe-capture"}'
grep 'call-controls refused {"capture"' /tmp/card.log | tail -1 | grep -q '"capture":"NotAllowedError"' \
  || fail "the card's page was not refused the microphone"
grep -q "call-controls permission Microphone denied" /tmp/card.log || fail "the app did not deny the card's request"
# The editor has the keyboard.
/tmp/card-ci type "abc"
expect /tmp/editor.log "editor text=abc"

# The pointer over the card (the app polls it: a panel that is never key gets no hover): its controls appear
# below it, and the window grows to hold them.
/tmp/card-ci move $((X + 160)) $((Y + 30))
expect /tmp/card.log "call-controls pointer inside=Some(true) app_active=false"
sleep 1.5
card
[ "$H" -ge 110 ] || fail "the card did not grow with its controls (h=$H)"
screencapture -x card-shots/2-near-you-talking.png

# A click on the title (no move) opens the conversations below the controls; a click in the editor closes it.
/tmp/card-ci click $((X + 90)) $((Y + 31))
sleep 1.5
card
[ "$H" -ge 200 ] || fail "the conversations did not open (h=$H)"
screencapture -x card-shots/3-conversations.png
FROM=$(line /tmp/card.log)
/tmp/card-ci click 200 700
expect /tmp/card.log "call-controls outside click" "$FROM"
editor_active "after a click in it"

# A drag from the card (not a button) moves it; let go, it is dropped and remembered. The app stays inactive.
/tmp/card-ci move $((X + 160)) $((Y + 30))
expect /tmp/card.log "call-controls pointer inside=Some(true)" "$FROM"
sleep 1
card
BEFORE="$X $Y"
FROM=$(line /tmp/card.log)
/tmp/card-ci drag $((X + 40)) $((Y + 39)) -300 200
expect /tmp/card.log "call-controls dropped at .* app_active=false" "$FROM"
sleep 1
card
[ "$X $Y" != "$BEFORE" ] || fail "the card did not move"
screencapture -x card-shots/4-dragged.png
editor_active "after a drag"

# Its mute button, clicked: the command reaches the room, the app never becomes active, and typing still
# reaches the editor. Muted, then unmuted again (the probe then moves on).
for state in muted unmuted; do
  /tmp/card-ci move $((X + 160)) $((Y + 30)); sleep 1; card
  FROM=$(line /tmp/card.log)
  /tmp/card-ci click $((X + 66)) $((Y + H - 37))
  expect /tmp/card.log "call-controls run toggle-mute app_active=false" "$FROM"
  expect /tmp/card.log "probe-card command toggle-mute" "$FROM"
  sleep 1
  screencapture -x "card-shots/5-$state.png"
  editor_active "after mute ($state)"
done
/tmp/card-ci type "def"
expect /tmp/editor.log "editor text=abc.*def"
editor_active "at the end of the clicks"

# The other states, near (controls) and away (at rest).
expect /tmp/card.log "probe-card phase=working"; sleep 2; screencapture -x card-shots/6-near-agent-working.png
expect /tmp/card.log "probe-card phase=speaking"; sleep 2; screencapture -x card-shots/7-near-agent-speaking.png
FROM=$(line /tmp/card.log)
/tmp/card-ci move 200 700
expect /tmp/card.log "call-controls pointer inside=Some(false) app_active=false" "$FROM"
expect /tmp/card.log "probe-card phase=muted"; sleep 2; screencapture -x card-shots/8-rest-muted-agent-speaking.png
expect /tmp/card.log "probe-card phase=reconnecting"; sleep 2; screencapture -x card-shots/9-rest-reconnecting.png
# Its hang-up button ends the call; the card goes with it (after that, not from an earlier hide).
if grep -q "call-controls hidden" /tmp/card.log; then fail "the card hid before the call ended"; fi
card
/tmp/card-ci move $((X + 160)) $((Y + 30)); sleep 1.5; card
FROM=$(line /tmp/card.log)
/tmp/card-ci click $((X + W - 45)) $((Y + H - 37))
expect /tmp/card.log "probe-card command hang-up" "$FROM"
HUNG_UP=$(grep -n "probe-card command hang-up" /tmp/card.log | tail -1 | cut -d: -f1)
expect /tmp/card.log "call-controls hidden" "${HUNG_UP:-$FROM}"
expect /tmp/card.log "probe-card done" "$FROM"
if grep -q "editor active=false" /tmp/editor.log; then fail "the editor stopped being the active app"; fi
# Nor the keyboard: its window stays key from full screen on (a loss put right later still counts).
if sed -n '/editor fullscreen=true key=true/,$p' /tmp/editor.log | grep -q "editor key=false"; then
  fail "the editor's window lost the keyboard: $(grep 'editor key=false' /tmp/editor.log | head -1)"
fi
kill "$PID" "$EDITOR" || true
if [ -n "$FAILED" ]; then echo "failed:$FAILED"; exit 1; fi
