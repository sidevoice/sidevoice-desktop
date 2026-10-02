#!/bin/sh
set -eu

case "${1-} ${2-}" in
  "--version --json")
    printf '%s\n' '{"ok":true,"fixture":"sidevoice-r4-c-package-fixture-v1"}'
    ;;
  "metadata --json")
    printf '%s\n' '{"ok":true,"fixture":"sidevoice-r4-c-package-fixture-v1"}'
    ;;
  *)
    printf '%s\n' '{"ok":false,"error":{"key":"fixture.unsupported"}}'
    exit 2
    ;;
esac
