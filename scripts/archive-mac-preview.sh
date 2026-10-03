#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
APP="$ROOT/src-tauri/target/packages/production/Sidevoice.app"
EVIDENCE="$ROOT/dist/build-evidence.json"
PREVIEW_DIR="$ROOT/src-tauri/target/previews"
ARCHIVE_NAME="Sidevoice-dev-macos-arm64.zip"
SIDECAR_NAME="$ARCHIVE_NAME.sha256"
ARCHIVE="$PREVIEW_DIR/$ARCHIVE_NAME"
SIDECAR="$PREVIEW_DIR/$SIDECAR_NAME"
APP_EXECUTABLE="Contents/MacOS/sidevoice-desktop"

if [[ ! -d "$APP" ]]; then
  echo "Verified production app is missing: $APP" >&2
  exit 1
fi
if [[ ! -f "$EVIDENCE" ]]; then
  echo "Production build evidence is missing: $EVIDENCE" >&2
  exit 1
fi
if [[ ! -x "$APP/$APP_EXECUTABLE" ]]; then
  echo "Production app executable is missing: $APP/$APP_EXECUTABLE" >&2
  exit 1
fi

EXPECTED_SHA256="$(node -e '
const fs = require("node:fs");
const evidence = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
process.stdout.write(String(evidence.app_executable_sha256 ?? ""));
' "$EVIDENCE")"
if [[ ! "$EXPECTED_SHA256" =~ ^[a-f0-9]{64}$ ]]; then
  echo "Build evidence has no valid app_executable_sha256" >&2
  exit 1
fi

mkdir -p "$PREVIEW_DIR"
rm -f "$ARCHIVE" "$SIDECAR"
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$APP" "$ARCHIVE"
(
  cd "$PREVIEW_DIR"
  /usr/bin/shasum -a 256 "$ARCHIVE_NAME" > "$SIDECAR_NAME"
)

VERIFY_DIR="$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/sidevoice-preview.XXXXXX")"
trap 'rm -rf "$VERIFY_DIR"' EXIT
/usr/bin/ditto -x -k "$ARCHIVE" "$VERIFY_DIR"
EXTRACTED_APP="$VERIFY_DIR/Sidevoice.app"
if [[ ! -d "$EXTRACTED_APP" ]]; then
  echo "Archive did not contain Sidevoice.app at its root" >&2
  exit 1
fi
/usr/bin/codesign --verify --deep --strict "$EXTRACTED_APP"
ACTUAL_SHA256="$(/usr/bin/shasum -a 256 "$EXTRACTED_APP/$APP_EXECUTABLE" | /usr/bin/awk '{print $1}')"
if [[ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]]; then
  echo "Archived app executable does not match build evidence" >&2
  exit 1
fi

echo "Development app archive verified: $ARCHIVE"
echo "SHA-256 sidecar: $SIDECAR"
