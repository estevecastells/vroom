#!/bin/bash
# Build the macOS release artifacts into dist/:
#   Vroom-macos-arm64.zip     the menu bar app (the vroom CLI is inside)
#   vroom-macos-arm64.tar.gz  the CLI on its own
# The Windows CLI (vroom-windows-x64.zip) is built by the release workflow on Windows.
# Asset names carry no version so that .../releases/latest/download/<name> always works.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
export MACOSX_DEPLOYMENT_TARGET=14.0

scripts/build-app.sh
rm -rf dist
mkdir -p dist/stage/vroom
cp target/release/vroom LICENSE dist/stage/vroom/
tar -C dist/stage -czf dist/vroom-macos-arm64.tar.gz vroom
ditto -c -k --sequesterRsrc --keepParent build/Vroom.app dist/Vroom-macos-arm64.zip
rm -rf dist/stage
ls -la dist
