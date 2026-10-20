#!/usr/bin/env bash
# Builds "Kalkan Monitor.app" and the disk image: target/macos/KalkanMonitor-<version>.dmg
#
# Usage (on a Mac):  ./macos/build.sh [version]        e.g. ./macos/build.sh 0.1.0
#
# Requires Rust and the Xcode command line tools (clang, lipo, sips, iconutil,
# codesign, hdiutil). This is the same script GitHub Actions runs
# (.github/workflows/release.yml).
#
# The app is a universal binary (Apple Silicon + Intel) unless
# KALKAN_MAC_TARGETS lists fewer Rust targets, e.g. KALKAN_MAC_TARGETS=aarch64-apple-darwin
# for a quick local build. It is signed ad hoc, not notarized: see README.md for
# the first-launch steps.
set -euo pipefail

VERSION="${1:-0.0.0}"
if ! [[ "$VERSION" =~ ^[0-9]+(\.[0-9]+){1,2}$ ]]; then
  echo "version must be numeric, like 0.1.0 (got '$VERSION')" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
OUT="target/macos"
APP="$OUT/Kalkan Monitor.app"
TARGETS="${KALKAN_MAC_TARGETS:-aarch64-apple-darwin x86_64-apple-darwin}"
export MACOSX_DEPLOYMENT_TARGET=11.0   # matches LSMinimumSystemVersion in Info.plist.in

rm -rf "$OUT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# One binary per architecture, then merged.
bins=()
for t in $TARGETS; do
  rustup target add "$t"
  cargo build --release -p kalkan-ui --target "$t"
  bins+=("target/$t/release/kalkan-monitor")
done
if [ "${#bins[@]}" -gt 1 ]; then
  lipo -create -output "$APP/Contents/MacOS/kalkan-monitor" "${bins[@]}"
else
  cp "${bins[0]}" "$APP/Contents/MacOS/kalkan-monitor"
fi
chmod 755 "$APP/Contents/MacOS/kalkan-monitor"
lipo -info "$APP/Contents/MacOS/kalkan-monitor"

# Info.plist with the version filled in.
sed "s/@VERSION@/$VERSION/g" macos/Info.plist.in > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"

# Icon: .icns generated from the 1024 px PNG.
ICONSET="$OUT/kalkan-monitor.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" assets/icons/kalkan-monitor-1024.png --out "$ICONSET/icon_${s}x${s}.png" > /dev/null
  sips -z "$((s * 2))" "$((s * 2))" assets/icons/kalkan-monitor-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" > /dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/kalkan-monitor.icns"
rm -rf "$ICONSET"

# Ad-hoc signature (no developer account): enough for the app to run once the
# user has approved it in Gatekeeper.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

# Disk image: the app next to a shortcut to /Applications.
STAGE="$OUT/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG="$OUT/KalkanMonitor-$VERSION.dmg"
hdiutil create -volname "Kalkan Monitor" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
echo "Built $DMG"
