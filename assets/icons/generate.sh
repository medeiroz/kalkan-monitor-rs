#!/usr/bin/env bash
# Regenerates every icon file from kalkan-monitor.svg (the single source).
# Renders with headless Chrome (set CHROME to its path if not on macOS default),
# because ImageMagick's own SVG renderer does not draw the arcs. Needs ImageMagick.
#
#   kalkan-monitor.png       256 px   Linux (.deb) and the app window
#   kalkan-monitor-1024.png  1024 px  macOS .icns source
#   kalkan-monitor.ico       16-256   Windows exe + installer
#   tray-32.rgba             32x32    raw RGBA, system tray (Windows/Linux)
set -euo pipefail
cd "$(dirname "$0")"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT

CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
cat > "$T/icon.html" <<HTML
<style>html,body{margin:0;background:transparent}img{display:block;width:1024px;height:1024px}</style>
<img src="file://$PWD/kalkan-monitor.svg">
HTML
"$CHROME" --headless --disable-gpu --hide-scrollbars --default-background-color=00000000 \
  --window-size=1024,1024 --screenshot="$T/1024.png" "file://$T/icon.html" > /dev/null 2>&1

magick "$T/1024.png" PNG32:kalkan-monitor-1024.png
magick "$T/1024.png" -filter Lanczos -resize 256x256 PNG32:kalkan-monitor.png
magick "$T/1024.png" -filter Lanczos -resize 32x32 -depth 8 rgba:tray-32.rgba
sizes=(16 24 32 48 64 128 256)
for s in "${sizes[@]}"; do magick "$T/1024.png" -filter Lanczos -resize "${s}x${s}" PNG32:"$T/$s.png"; done
magick $(for s in "${sizes[@]}"; do echo "$T/$s.png"; done) kalkan-monitor.ico
