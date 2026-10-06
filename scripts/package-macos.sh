#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

arch="${1:-}"
output_dir="${2:-dist}"
case "$arch" in
  arm64|x86_64) ;;
  *) echo "Usage: $0 <arm64|x86_64> [output-directory]" >&2; exit 1 ;;
esac

if [[ "$(uname -s)" != Darwin ]]; then
  echo "This script requires macOS." >&2
  exit 1
fi

if [[ "$(uname -m)" != "$arch" ]]; then
  echo "Requested $arch package, but this machine is $(uname -m). Build on a matching runner." >&2
  exit 1
fi

version="${STARTER_VERSION:-$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)}"
build_number="${STARTER_BUILD:-${GITHUB_RUN_NUMBER:-1}}"
bundle="$root/target/release/Starter.app"
mkdir -p "$output_dir"

if [[ -n "${CODESIGN_IDENTITY:-}" || -n "${APPLE_CERTIFICATE_P12_BASE64:-}" || -n "${APPLE_CERTIFICATE_PASSWORD:-}" || -n "${APPLE_ID:-}" || -n "${APPLE_TEAM_ID:-}" || -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ]]; then
  for variable in CODESIGN_IDENTITY APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_ID APPLE_TEAM_ID APPLE_APP_SPECIFIC_PASSWORD; do
    if [[ -z "${!variable:-}" ]]; then
      echo "$variable must be set when Apple signing or notarization is enabled." >&2
      exit 1
    fi
  done
fi

STARTER_VERSION="$version" STARTER_BUILD="$build_number" \
  bash "$root/scripts/bundle-macos.sh" --release

staging="$(mktemp -d "${TMPDIR:-/tmp}/starter-dmg.XXXXXX")"
cleanup() { rm -rf "$staging"; }
trap cleanup EXIT
cp -R "$bundle" "$staging/Starter.app"
ln -s /Applications "$staging/Applications"

dmg="$output_dir/Starter-$version-macos-$arch.dmg"
rm -f "$dmg"
hdiutil create -volname "Starter $version" -srcfolder "$staging" -ov -format UDZO "$dmg" >/dev/null

if [[ -n "${APPLE_ID:-}" || -n "${APPLE_TEAM_ID:-}" || -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ]]; then
  if [[ -z "${APPLE_ID:-}" || -z "${APPLE_TEAM_ID:-}" || -z "${APPLE_APP_SPECIFIC_PASSWORD:-}" ]]; then
    echo "APPLE_ID, APPLE_TEAM_ID, and APPLE_APP_SPECIFIC_PASSWORD must be set together." >&2
    exit 1
  fi
  xcrun notarytool submit "$dmg" \
    --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --wait
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
fi

echo "Created $dmg"
