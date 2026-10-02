#!/bin/sh
# bundle.sh <binary> <version> <out-dir> <arch>
set -eu
BIN="${1:?missing binary}"
VERSION="${2:?missing version}"
OUT="${3:?missing output directory}"
ARCH="${4:?missing architecture}"
case "$ARCH" in x86_64 | aarch64) ;; *) echo "Unsupported architecture: $ARCH" >&2; exit 1;; esac
HERE="$(cd "$(dirname "$0")" && pwd)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$OUT"
cp "$BIN" "$STAGE/skelly"
chmod +x "$STAGE/skelly"
cp "$HERE/skelly.png" "$STAGE/skelly.png"
sh "$HERE/../voice-assets.sh" "$STAGE/share/skelly/pi"
tar czf "$OUT/skelly-v$VERSION-linux-$ARCH.tar.gz" -C "$STAGE" skelly skelly.png share
