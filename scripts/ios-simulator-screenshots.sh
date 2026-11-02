#!/usr/bin/env bash
# Boots an iPhone Pro Max simulator, installs the simulator app (a zip of the .app, `cargo xtask mobile build ios`),
# launches it and keeps, in the output directory, a screenshot in light and in dark appearance and the app's log.
# Usage: scripts/ios-simulator-screenshots.sh APP.zip BUNDLE_ID OUT_DIR
set -euo pipefail

zip=$1 bundle=$2 out=$3
mkdir -p "$out"
work=$(mktemp -d)
unzip -q "$zip" -d "$work"
app=$(find "$work" -maxdepth 1 -name '*.app' | head -1)

# The newest iPhone Pro Max the runner has a device type for, on the newest iOS runtime it has.
type=$(xcrun simctl list devicetypes | grep -E 'iPhone [0-9]+ Pro Max \(' | sed -E 's/.*\((com\.apple[^)]*)\)/\1/' | sort -V | tail -1)
runtime=$(xcrun simctl list runtimes available | grep -E '^iOS' | sed -E 's/.* - (com\.apple[^ ]*)$/\1/' | sort -V | tail -1)
echo "device type $type, runtime $runtime" | tee "$out/device.txt"
udid=$(xcrun simctl create sidevoice-shot "$type" "$runtime")
xcrun simctl boot "$udid"
xcrun simctl bootstatus "$udid" -b >/dev/null
xcrun simctl install "$udid" "$app"

for mode in light dark; do
  xcrun simctl ui "$udid" appearance "$mode"
  xcrun simctl terminate "$udid" "$bundle" 2>/dev/null || true
  xcrun simctl launch --console-pty "$udid" "$bundle" >"$out/console-$mode.txt" 2>&1 &
  launch=$!
  sleep 25
  xcrun simctl io "$udid" screenshot "$out/screen-$mode.png"
  kill "$launch" 2>/dev/null || true
  xcrun simctl terminate "$udid" "$bundle" 2>/dev/null || true
done

# The app's own log files, if the log plugin wrote any, and the system log of the app's process.
data=$(xcrun simctl get_app_container "$udid" "$bundle" data)
mkdir -p "$out/app-data"
cp -R "$data/Library/Logs" "$out/app-data/" 2>/dev/null || true
xcrun simctl spawn "$udid" log show --last 5m --style compact --predicate 'process == "Sidevoice" OR process == "sidevoice-mobile"' >"$out/system-log.txt" 2>&1 || true
xcrun simctl shutdown "$udid" || true
