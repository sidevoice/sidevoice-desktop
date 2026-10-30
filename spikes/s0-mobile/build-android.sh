#!/usr/bin/env bash
# S0 spike: the signed Android APK (arm64), as built in the pod. Needs ANDROID_HOME, NDK_HOME (r28+) and JAVA_HOME
# (21) set, Rust with aarch64-linux-android, and the upload keystore: S0_KEYSTORE (PKCS12, alias `upload`) and
# S0_KEYSTORE_PASS_FILE (its password, in a file; never on the command line). Writes $1 (default: s0-android.apk).
set -euo pipefail
cd "$(dirname "$0")"
out=$(realpath -m "${1:-s0-android.apk}")
: "${S0_KEYSTORE:?the upload keystore (.p12)}" "${S0_KEYSTORE_PASS_FILE:?a file with its password}"
build_tools=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
npm ci
[ -d src-tauri/gen/android ] || npx tauri android init --ci
npx tauri android build --apk --target aarch64 --ci
apk=src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# 16 KB page alignment for the native library (Play's rule; NDK r28+ links it so).
"$build_tools/zipalign" -P 16 -f 4 "$apk" "$work/aligned.apk"
"$build_tools/apksigner" sign --ks "$S0_KEYSTORE" --ks-type PKCS12 --ks-key-alias upload \
  --ks-pass "file:$S0_KEYSTORE_PASS_FILE" --out "$out" "$work/aligned.apk"
"$build_tools/apksigner" verify "$out"
echo "signed APK: $out"
