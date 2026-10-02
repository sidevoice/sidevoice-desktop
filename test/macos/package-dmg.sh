#!/usr/bin/env bash
# Run from the repository root on a disposable macOS test account.
set -euo pipefail
PACKAGE_TEMP=$(mktemp -d)
MNT=""
trap '[ -z "$MNT" ] || hdiutil detach "$MNT"; rm -rf "$PACKAGE_TEMP"' EXIT
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
APP="${APP:-src-tauri/target/packages/production/Sidevoice.app}"
python3 -m venv "$PACKAGE_TEMP/dmgbuild"
"$PACKAGE_TEMP/dmgbuild/bin/pip" install --quiet dmgbuild==1.6.7
# One TIFF with the 1x and 2x drawings: Finder picks the one for the screen.
tiffutil -cathidpicheck src-tauri/dmg/background.png src-tauri/dmg/background@2x.png -out "$PACKAGE_TEMP/background.tiff"
mkdir -p dist
DMG="dist/Sidevoice_${VERSION}_aarch64.dmg"
"$PACKAGE_TEMP/dmgbuild/bin/dmgbuild" -s scripts/dmg-settings.py \
  -D app="$APP" -D background="$PACKAGE_TEMP/background.tiff" \
  -D layout=src-tauri/dmg/layout.json -D icon=src-tauri/icons/icon.icns \
  Sidevoice "$DMG"
hdiutil verify "$DMG"
# Check the app as shipped, inside the image, not the one in the build tree.
MNT="$PACKAGE_TEMP/mount"
mkdir "$MNT"
hdiutil attach -readonly -nobrowse -mountpoint "$MNT" "$DMG"
ls -la "$MNT"
test -s "$MNT/.background.tiff"
test -L "$MNT/Applications"
test -s "$MNT/.DS_Store"
test -s "$MNT/.VolumeIcon.icns"
codesign --verify --deep --strict --verbose=2 "$MNT/Sidevoice.app"
ditto "$MNT/Sidevoice.app" "$PACKAGE_TEMP/Sidevoice-moved.app"
node scripts/verify-packaged-connector.mjs "$PACKAGE_TEMP/Sidevoice-moved.app/Contents/Resources/resources/sidevoice"
codesign -d --entitlements - --xml "$MNT/Sidevoice.app" | grep -q "com.apple.security.device.audio-input"
plutil -extract NSMicrophoneUsageDescription raw "$MNT/Sidevoice.app/Contents/Info.plist"
hdiutil detach "$MNT"
MNT=""
cp docs/FIRST_OPEN.txt dist/FIRST_OPEN.txt
(cd dist && shasum -a 256 *.dmg | tee SHA256SUMS.txt)
