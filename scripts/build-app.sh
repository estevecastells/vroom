#!/bin/bash
# Build build/Vroom.app (menu bar recorder + the vroom CLI). Works with Command Line Tools only.
# Signed with VROOM_SIGN_IDENTITY, else "Vroom Local Signing" if it exists
# (scripts/make-signing-cert.sh), else ad hoc. A stable identity lets macOS keep
# the Screen Recording permission across rebuilds; ad-hoc signatures change every build.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
app="build/Vroom.app"

echo "==> Building the vroom CLI (release)"
cargo build --release --locked -p vroom

echo "==> Building the Swift app (release)"
(cd app && swift build -c release --product Vroom)
swift_bin="$(cd app && swift build -c release --show-bin-path)/Vroom"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$swift_bin" "$app/Contents/MacOS/Vroom"
cp target/release/vroom "$app/Contents/Resources/vroom"
cp app/Resources/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
# Translations: one <lang>.lproj per language (see app/Resources/en.lproj/Localizable.strings).
localizations=""
for lproj in app/Resources/*.lproj; do
  cp -R "$lproj" "$app/Contents/Resources/"
  localizations+="<string>$(basename "$lproj" .lproj)</string>"
done
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>io.github.estevecastells.vroom.app</string>
  <key>CFBundleName</key><string>Vroom</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleLocalizations</key><array>${localizations}</array>
  <key>CFBundleDisplayName</key><string>Vroom</string>
  <key>CFBundleExecutable</key><string>Vroom</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSCameraUsageDescription</key><string>Vroom shows your camera in a bubble inside the recording.</string>
  <key>NSMicrophoneUsageDescription</key><string>Vroom records your voice along with the screen.</string>
</dict>
</plist>
PLIST
identity="${VROOM_SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  # scripts/make-signing-cert.sh creates a stable local identity; fall back to ad hoc.
  if security find-certificate -c "Vroom Local Signing" >/dev/null 2>&1; then identity="Vroom Local Signing"; else identity="-"; fi
fi
# Hardened runtime: other processes can't inject code to borrow the app's
# Screen Recording, camera and microphone permissions.
codesign --force --options runtime --sign "$identity" "$app/Contents/Resources/vroom" >/dev/null
codesign --force --options runtime --entitlements app/Vroom.entitlements --sign "$identity" "$app" >/dev/null
echo "==> Done: $root/$app"
