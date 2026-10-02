#!/bin/sh
# Copy only the companion's runtime files. Never installs it into Pi or downloads engines.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:?usage: voice-assets.sh <destination>}"
mkdir -p "$OUT"
for FILE in index.ts package.json README.md; do
  cp "$HERE/../integrations/pi/$FILE" "$OUT/$FILE"
done
cp "$HERE/../LICENSE-MIT" "$HERE/../LICENSE-APACHE" "$OUT/"
