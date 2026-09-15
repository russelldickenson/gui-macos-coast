#!/bin/sh
# Builds Coast.app: cargo-builds the binary, assembles a real app bundle
# (Info.plist, icon), and ad-hoc codesigns it so macOS treats it as a
# normal application (Dock icon, menu bar identity, Cmd-Tab, etc).
#
# Usage: packaging/macos/bundle.sh [debug|release]   (default: release)
set -eu

PROFILE="${1:-release}"
case "$PROFILE" in
    debug) CARGO_FLAGS="" ;;
    release) CARGO_FLAGS="--release" ;;
    *) echo "usage: $0 [debug|release]" >&2; exit 1 ;;
esac

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

cargo build $CARGO_FLAGS --package coast

BIN="target/$PROFILE/coast"
if [ ! -x "$BIN" ]; then
    echo "build did not produce $BIN" >&2
    exit 1
fi

APP="target/$PROFILE/bundle/Coast.app"
CONTENTS="$APP/Contents"
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

cp "$BIN" "$CONTENTS/MacOS/coast"
cp assets/icon/AppIcon.icns "$CONTENTS/Resources/AppIcon.icns"
sed "s/__VERSION__/$VERSION/g" packaging/macos/Info.plist.in > "$CONTENTS/Info.plist"

codesign --force --deep --sign - "$APP"

echo "built $APP"
