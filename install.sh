#!/bin/sh
# Install the latest Skelly release.
#   curl -fsSL https://zottiben.github.io/skelly/install.sh | sh
#
# macOS: installs Skelly.app (a universal build) into /Applications and drops a
# `skelly` launcher on your PATH. Linux: installs the `skelly` binary on your PATH
# plus a .desktop entry. Windows is out of scope for v0.1.
set -eu

REPO="zottiben/skelly"

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
  darwin | linux) ;;
  *) echo "Unsupported OS: $OS (Skelly targets macOS and Linux)" >&2; exit 1 ;;
esac

# Pick a bin dir on PATH without needing sudo when possible.
if echo "$PATH" | tr ':' '\n' | grep -qx "$HOME/.local/bin"; then
  BIN_DIR="$HOME/.local/bin"
else
  BIN_DIR="/usr/local/bin"
fi

# --- resolve the latest release tag ---------------------------------------------
VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
  | grep '"tag_name"' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -z "$VERSION" ]; then
  echo "Could not determine the latest release. Is one published yet?" >&2
  exit 1
fi
VERSION_NUM="${VERSION#v}"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"

# --- asset name (must match .github/workflows/release.yml) -----------------------
if [ "$OS" = "darwin" ]; then
  FILENAME="Skelly-v${VERSION_NUM}-macos-universal.tar.gz"
else
  case "$ARCH" in
    x86_64 | amd64) LARCH="x86_64" ;;
    arm64 | aarch64) LARCH="aarch64" ;;
    *) echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
  esac
  FILENAME="skelly-v${VERSION_NUM}-linux-${LARCH}.tar.gz"
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading Skelly ${VERSION} for ${OS}..."
curl -fsSL "${BASE}/${FILENAME}" -o "${TMPDIR}/${FILENAME}"

# --- verify checksum (best-effort: only if published and a hasher is available) --
if curl -fsSL "${BASE}/checksums.txt" -o "${TMPDIR}/checksums.txt" 2>/dev/null; then
  expected="$(grep " ${FILENAME}\$" "${TMPDIR}/checksums.txt" | awk '{print $1}')"
  if [ -n "$expected" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      actual="$(sha256sum "${TMPDIR}/${FILENAME}" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
      actual="$(shasum -a 256 "${TMPDIR}/${FILENAME}" | awk '{print $1}')"
    else
      actual=""
    fi
    if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
      echo "Checksum mismatch for ${FILENAME}" >&2
      exit 1
    fi
  fi
fi

tar xzf "${TMPDIR}/${FILENAME}" -C "$TMPDIR"

install_bin() {
  # $1 = source binary path; place it at $BIN_DIR/skelly (sudo only if needed).
  if mkdir -p "$BIN_DIR" 2>/dev/null && [ -w "$BIN_DIR" ]; then
    mv -f "$1" "$BIN_DIR/skelly"
    chmod +x "$BIN_DIR/skelly"
  else
    echo "Installing to ${BIN_DIR} (requires sudo)..."
    sudo mkdir -p "$BIN_DIR"
    sudo mv -f "$1" "$BIN_DIR/skelly"
    sudo chmod +x "$BIN_DIR/skelly"
  fi
}

if [ "$OS" = "darwin" ]; then
  # Install the app bundle; prefer /Applications (writable for admins), else ~/Applications.
  if [ -w "/Applications" ]; then
    APP_DIR="/Applications"
  else
    APP_DIR="$HOME/Applications"
    mkdir -p "$APP_DIR"
  fi
  rm -rf "${APP_DIR}/Skelly.app"
  mv "${TMPDIR}/Skelly.app" "${APP_DIR}/Skelly.app"
  # curl downloads carry no quarantine, but clear it anyway in case of a re-host.
  xattr -dr com.apple.quarantine "${APP_DIR}/Skelly.app" 2>/dev/null || true

  # A `skelly` launcher on PATH so it opens from any terminal too.
  SKELLY_BIN="${APP_DIR}/Skelly.app/Contents/MacOS/skelly"
  if mkdir -p "$BIN_DIR" 2>/dev/null && [ -w "$BIN_DIR" ]; then
    ln -sf "$SKELLY_BIN" "$BIN_DIR/skelly"
  else
    echo "Linking 'skelly' into ${BIN_DIR} (requires sudo)..."
    sudo mkdir -p "$BIN_DIR" && sudo ln -sf "$SKELLY_BIN" "$BIN_DIR/skelly"
  fi
  echo "Installed Skelly ${VERSION} to ${APP_DIR}/Skelly.app"
  echo "Launch it from Spotlight/Launchpad, or run 'skelly' in a terminal."
else
  install_bin "${TMPDIR}/skelly"
  # Desktop entry + icon for the application menu (best-effort, user-scoped).
  APPS="$HOME/.local/share/applications"
  ICONS="$HOME/.local/share/icons/hicolor/256x256/apps"
  if mkdir -p "$APPS" "$ICONS" 2>/dev/null; then
    [ -f "${TMPDIR}/skelly.png" ] && cp "${TMPDIR}/skelly.png" "${ICONS}/skelly.png"
    cat > "${APPS}/skelly.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Skelly
Comment=A barebones, keyboard-driven terminal emulator
Exec=${BIN_DIR}/skelly
Icon=skelly
Terminal=false
Categories=Development;System;TerminalEmulator;
EOF
  fi
  echo "Installed Skelly ${VERSION} to ${BIN_DIR}/skelly"
fi

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$BIN_DIR"; then
  echo
  echo "Note: ${BIN_DIR} is not on your PATH. Add it, e.g.:"
  echo "  export PATH=\"${BIN_DIR}:\$PATH\""
fi
