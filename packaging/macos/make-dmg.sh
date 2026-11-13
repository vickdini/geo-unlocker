#!/usr/bin/env bash
# Build "Geo Unlocker.app" and a .dmg on macOS.
#   packaging/macos/make-dmg.sh <rust-target> <tor-platform> <version>
#   e.g. packaging/macos/make-dmg.sh aarch64-apple-darwin macos-aarch64 0.2.0
set -euo pipefail
TARGET="$1" TORPLAT="$2" VERSION="$3"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist/macos-$TARGET"
APP="$OUT/Geo Unlocker.app"
rm -rf "$OUT" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cargo build --release -p geo-unlocker --target "$TARGET"
cp "$ROOT/target/$TARGET/release/geo-unlocker" "$APP/Contents/MacOS/geo-unlocker"

# Tor Expert Bundle (unmodified, minus pluggable transports) -> Resources/tor
python3 "$ROOT/packaging/fetch_tor.py" "$TORPLAT" "$APP/Contents/Resources/tor"
cp "$ROOT/crates/desktop/static/NOTICE.md" "$APP/Contents/Resources/"

# Icon
ICONSET="$OUT/AppIcon.iconset"; mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  cp "$ROOT/packaging/icons/geo-unlocker-$s.png" "$ICONSET/icon_${s}x${s}.png"
  d=$((s * 2)); [ -f "$ROOT/packaging/icons/geo-unlocker-$d.png" ] && cp "$ROOT/packaging/icons/geo-unlocker-$d.png" "$ICONSET/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Geo Unlocker</string>
  <key>CFBundleDisplayName</key><string>Geo Unlocker</string>
  <key>CFBundleIdentifier</key><string>app.geounlocker.desktop</string>
  <key>CFBundleExecutable</key><string>geo-unlocker</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF

# Ad-hoc signature so macOS accepts the bundle structure (not notarized: users
# right-click > Open the first time, unless you sign with a Developer ID).
codesign --force --deep --sign - "$APP" || true

ARCH=$([ "$TARGET" = "aarch64-apple-darwin" ] && echo arm64 || echo x64)
DMG="$ROOT/dist/GeoUnlocker-$VERSION-macos-$ARCH.dmg"
ln -s /Applications "$OUT/Applications"
hdiutil create -volname "Geo Unlocker" -srcfolder "$OUT" -ov -format UDZO "$DMG"
echo "built $DMG"
