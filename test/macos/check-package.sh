#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -euo pipefail
CHECK_TEMP=$(mktemp -d)
trap 'rm -rf "$CHECK_TEMP"' EXIT
APP="${APP:-src-tauri/target/packages/production/Sidevoice.app}"
test -d "$APP"
echo "--- Info.plist"
plutil -p "$APP/Contents/Info.plist"
plutil -extract NSMicrophoneUsageDescription raw "$APP/Contents/Info.plist"
test "$(plutil -extract LSMinimumSystemVersion raw "$APP/Contents/Info.plist")" = "13.0"
echo "--- signature"
codesign --verify --deep --strict --verbose=2 "$APP"
SEA="$APP/Contents/Resources/resources/sidevoice"
test -x "$SEA"
cmp src-tauri/connector-pin.json "$APP/Contents/Resources/resources/connector-pin.json"
if [ "${CONNECTOR_FIXTURE:-false}" = "true" ]; then
  node scripts/verify-packaged-connector-fixture.mjs "$SEA" "$CONNECTOR_FIXTURE_EXPECTED"
else
  node scripts/verify-packaged-connector.mjs "$SEA"
fi
codesign -dvvv "$APP" 2>&1 | tee "$CHECK_TEMP/sig.txt"
grep -q "Signature=adhoc" "$CHECK_TEMP/sig.txt"
grep -q "runtime" "$CHECK_TEMP/sig.txt"
echo "--- entitlements"
codesign -d --entitlements - --xml "$APP" > "$CHECK_TEMP/ent.xml"
cat "$CHECK_TEMP/ent.xml"; echo
grep -q "com.apple.security.device.audio-input" "$CHECK_TEMP/ent.xml"
grep -q "com.apple.security.cs.disable-library-validation" "$CHECK_TEMP/ent.xml"
echo "--- icon"
test "$(plutil -extract CFBundleIconFile raw "$APP/Contents/Info.plist")" = "icon.icns"
iconutil --convert iconset --output "${CHECK_TEMP}/icon.iconset" "$APP/Contents/Resources/icon.icns"
ls "${CHECK_TEMP}/icon.iconset"
for f in 16x16 16x16@2x 32x32 32x32@2x 128x128 128x128@2x 256x256 256x256@2x 512x512 512x512@2x; do
  test -s "${CHECK_TEMP}/icon.iconset/icon_$f.png"
done
sips -g pixelWidth "${CHECK_TEMP}/icon.iconset/icon_512x512@2x.png" | grep -q 1024
# How macOS itself decodes every entry, for the record (uploaded with the .dmg window screenshots).
mkdir -p dmg-screens && cp -R "${CHECK_TEMP}/icon.iconset" dmg-screens/
test "$(plutil -extract CFBundleName raw "$APP/Contents/Info.plist")" = "Sidevoice"
echo "--- architecture"
lipo -archs "$APP/Contents/MacOS/sidevoice-desktop" | tee /dev/stderr | grep -q arm64
echo "--- CI probe feature boundary"
if [ "${EXPECT_CI_PROBE:-false}" = "true" ]; then
  grep -q "SIDEVOICE_DEBUG_LOCAL_HOST_DOGFOOD" "$APP/Contents/MacOS/sidevoice-desktop"
  grep -q "local-host-dogfood" "$APP/Contents/MacOS/sidevoice-desktop"
else
  if grep -q "SIDEVOICE_DEBUG_PAGE" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the probe switch is in the release app"; exit 1; fi
  if grep -q "probe-engine" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the probe page is in the release app"; exit 1; fi
  if grep -q "probe-card" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the call controls probe is in the release app"; exit 1; fi
  if grep -q "SIDEVOICE_DEBUG_ROOM_FLOW" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the room-flow switch is in the release app"; exit 1; fi
  if grep -q "room-flow" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the room-flow script is in the release app"; exit 1; fi
  if grep -q "SIDEVOICE_DEBUG_IDLE_UNLOAD_SECS" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the idle-time switch is in the release app"; exit 1; fi
  if grep -Eq "SIDEVOICE_DEBUG_(REFUSE_LOAD|SLOW_TRANSCRIBE)" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "a fault switch is in the release app"; exit 1; fi
  if grep -Eq "SIDEVOICE_DEBUG_LOCAL_HOST_FLOW|local-host-flow" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the local-host flow is in the release app"; exit 1; fi
  if grep -Eq "SIDEVOICE_DEBUG_LOCAL_HOST_DOGFOOD|local-host-dogfood" "$APP/Contents/MacOS/sidevoice-desktop"; then echo "the R4 page driver is in the release app"; exit 1; fi
fi
