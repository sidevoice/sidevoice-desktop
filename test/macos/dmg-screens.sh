#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -uo pipefail
osa() { perl -e 'alarm shift; exec @ARGV' 30 osascript "$@"; }  # a permission prompt must not hang the job
mkdir -p dmg-screens
DMG=$(ls dist/*.dmg)
hdiutil attach -readonly "$DMG" | tee /tmp/attach.txt
VOL=$(grep -o '/Volumes/.*' /tmp/attach.txt | head -1)
for mode in false true; do
  osa -e "tell application \"System Events\" to tell appearance preferences to set dark mode to $mode" || true
  osa -e "tell application \"Finder\" to close every window" \
            -e "tell application \"Finder\" to open (POSIX file \"$VOL\" as alias)" \
            -e "tell application \"Finder\" to activate" || true
  sleep 4
  B=$(osa -e 'tell application "Finder" to get bounds of front window' | tr -d ' ')
  echo "window bounds ($mode): $B"
  IFS=, read -r X1 Y1 X2 Y2 <<< "${B:-0,28,1440,900}"
  screencapture -x -R"$X1,$((Y1 - 28)),$((X2 - X1)),$((Y2 - Y1 + 28))" "dmg-screens/dmg-dark-$mode.png" || screencapture -x "dmg-screens/dmg-dark-$mode-full.png"
done
osa -e 'tell application "Finder" to close every window' || true
hdiutil detach "$VOL" || true
ls -la dmg-screens
