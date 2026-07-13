#!/bin/sh
# Assemble and code-sign Skelly.app around a built `skelly` binary. macOS-only.
#
#   bundle.sh <binary> <version> <out-dir> [signing-identity]
#
# <binary>            path to the compiled `skelly` (ideally a universal lipo binary)
# <version>           release version, e.g. 0.1.0 (baked into Info.plist)
# <out-dir>           directory to create <out-dir>/Skelly.app in
# [signing-identity]  codesign identity. Default "-" = ad-hoc (no Apple account
#                     needed; required so the binary runs on Apple Silicon). Pass a
#                     "Developer ID Application: NAME (TEAMID)" to sign + harden for
#                     notarization (adds --options runtime --timestamp + entitlements).
set -eu

BIN="${1:?usage: bundle.sh <binary> <version> <out-dir> [identity]}"
VERSION="${2:?missing version}"
OUT="${3:?missing out-dir}"
IDENTITY="${4:--}"

HERE="$(cd "$(dirname "$0")" && pwd)"
APP="$OUT/Skelly.app"
CONTENTS="$APP/Contents"

rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

cp "$BIN" "$CONTENTS/MacOS/skelly"
chmod +x "$CONTENTS/MacOS/skelly"
cp "$HERE/skelly.icns" "$CONTENTS/Resources/skelly.icns"
sed "s/__VERSION__/$VERSION/g" "$HERE/Info.plist" > "$CONTENTS/Info.plist"
printf 'APPL????' > "$CONTENTS/PkgInfo"

if [ "$IDENTITY" = "-" ]; then
  # Ad-hoc: a bare signature so Gatekeeper/AMFI will launch it (curl downloads carry
  # no com.apple.quarantine, so no "unidentified developer" prompt on install).
  codesign --force --deep --sign - "$APP"
else
  # Developer ID: hardened runtime + entitlements + secure timestamp, ready for
  # `notarytool submit` + `stapler staple`.
  codesign --force --deep --options runtime --timestamp \
    --entitlements "$HERE/entitlements.plist" \
    --sign "$IDENTITY" "$APP"
fi

codesign --verify --deep --strict "$APP"
echo "built $APP (v$VERSION, signed with '${IDENTITY}')"
