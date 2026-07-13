#!/bin/sh
# Build packaging/macos/skelly.icns from the rendered 1024px master PNG.
# macOS-only (uses sips + iconutil, both built in). Run after render-icon.py.
set -e

cd "$(dirname "$0")"
SRC="skelly-icon-1024.png"
[ -f "$SRC" ] || { echo "missing $SRC - run render-icon.py first" >&2; exit 1; }

SET="skelly.iconset"
rm -rf "$SET"
mkdir -p "$SET"

# The ten sizes an .icns needs (base + @2x retina variants).
for spec in \
  "16 icon_16x16" "32 icon_16x16@2x" \
  "32 icon_32x32" "64 icon_32x32@2x" \
  "128 icon_128x128" "256 icon_128x128@2x" \
  "256 icon_256x256" "512 icon_256x256@2x" \
  "512 icon_512x512" "1024 icon_512x512@2x"; do
  px="${spec%% *}"
  name="${spec##* }"
  sips -z "$px" "$px" "$SRC" --out "$SET/$name.png" >/dev/null
done

iconutil -c icns "$SET" -o ../macos/skelly.icns
rm -rf "$SET"
echo "wrote packaging/macos/skelly.icns"
