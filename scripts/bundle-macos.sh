#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
case "${1:---release}" in
  --debug) profile=debug; build_args=(--locked) ;;
  --release) profile=release; build_args=(--locked --release) ;;
  *) echo "Usage: $0 [--debug|--release]" >&2; exit 1 ;;
esac
if [[ "$(uname -s)" != Darwin ]]; then
  echo "This script requires macOS." >&2
  exit 1
fi

cargo build "${build_args[@]}"
bundle="target/$profile/Starter.app"
mkdir -p "$bundle/Contents/MacOS"
cp "target/$profile/starter" "$bundle/Contents/MacOS/starter"
cp resources/macos/Info.plist "$bundle/Contents/Info.plist"
codesign --force --sign - "$bundle"
echo "Created $bundle"
