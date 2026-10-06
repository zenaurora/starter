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

version="${STARTER_VERSION:-$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)}"
build_number="${STARTER_BUILD:-${GITHUB_RUN_NUMBER:-1}}"
signing_identity="${CODESIGN_IDENTITY:-}"

cargo build "${build_args[@]}"
bundle="target/$profile/Starter.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "target/$profile/starter" "$bundle/Contents/MacOS/starter"
cp resources/macos/Info.plist "$bundle/Contents/Info.plist"
cp resources/icons/Starter.icns "$bundle/Contents/Resources/Starter.icns"

/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$bundle/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build_number" "$bundle/Contents/Info.plist"

if [[ -n "$signing_identity" ]]; then
  codesign --force --deep --options runtime --timestamp --sign "$signing_identity" "$bundle"
else
  codesign --force --deep --sign - "$bundle"
fi

codesign --verify --deep --strict "$bundle"
echo "Created $bundle (version $version, build $build_number)"
