#!/bin/bash
# Install or update Vroom in /Applications from the latest release:
#   curl -fsSL https://raw.githubusercontent.com/estevecastells/vroom/main/scripts/get.sh | bash
# Downloading with curl skips the quarantine flag that makes macOS refuse
# un-notarized apps; the checksum is verified against the release's SHA256SUMS.
set -euo pipefail
repo="estevecastells/vroom"
base="https://github.com/$repo/releases/latest/download"
[[ "$(uname -s)" == Darwin ]] || { echo "Vroom's app is for macOS; on Windows download vroom-windows-x64.zip." >&2; exit 1; }
[[ "$(uname -m)" == arm64 ]] || { echo "Vroom needs an Apple Silicon Mac." >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "==> Downloading Vroom"
curl -fsSL "$base/Vroom-macos-arm64.zip" -o "$tmp/Vroom-macos-arm64.zip"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
(cd "$tmp" && grep " Vroom-macos-arm64.zip\$" SHA256SUMS | shasum -a 256 -c -) >/dev/null
ditto -x -k "$tmp/Vroom-macos-arm64.zip" "$tmp"
pkill -x Vroom 2>/dev/null || true
rm -rf /Applications/Vroom.app
mv "$tmp/Vroom.app" /Applications/
xattr -dr com.apple.quarantine /Applications/Vroom.app 2>/dev/null || true
open /Applications/Vroom.app
echo "==> Vroom is in /Applications and running (icon in the menu bar)."
