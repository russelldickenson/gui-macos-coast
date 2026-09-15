#!/bin/sh
# Builds assets/icon/AppIcon.icns from assets/icon/AppIcon.png.
# Regenerate the PNG first with generate_icon.py if you change the artwork.
set -eu
cd "$(dirname "$0")"

SRC="AppIcon.png"
ICONSET="AppIcon.iconset"

rm -rf "$ICONSET"
mkdir "$ICONSET"

for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done

iconutil -c icns "$ICONSET" -o AppIcon.icns
rm -rf "$ICONSET"
echo "wrote $(pwd)/AppIcon.icns"
