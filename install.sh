#!/bin/sh
# Install or update Skelly.
#   curl -fsSL https://zottiben.github.io/skelly/install.sh | sh
# Once Skelly is installed, `skelly update` re-runs this same script for you.
#
# Options (pass them through `skelly update` too, e.g. `skelly update --check`):
#   --check           report whether an update is available, install nothing
#   --force           reinstall even when the latest version is already installed
#   --version <tag>   install a specific release tag (e.g. v0.1.8) instead of the latest
#   -h, --help        show this help
#
# macOS: installs Skelly.app (a universal build) into /Applications and drops a
# `skelly` launcher on your PATH. Linux: installs the `skelly` binary on your PATH
# plus a .desktop entry. Windows is out of scope for v0.1.
set -eu

REPO="zottiben/skelly"
# Records the installed tag so a later run can answer "already up to date" without
# downloading anything. `skelly update` also passes SKELLY_CURRENT_VERSION, which is
# authoritative (it is the running binary's own version) and wins over the receipt.
RECEIPT="${XDG_DATA_HOME:-$HOME/.local/share}/skelly/version"

usage() {
  cat <<'EOF'
Install or update Skelly.
  curl -fsSL https://zottiben.github.io/skelly/install.sh | sh
Once Skelly is installed, `skelly update` re-runs this same script for you.

Options:
  --check           report whether an update is available, install nothing
  --force           reinstall even when the latest version is already installed
  --version <tag>   install a specific release tag (e.g. v0.1.8) instead of the latest
  -h, --help        show this help
EOF
}

# Is version $1 strictly newer than $2? Both are plain numeric x.y.z, the only shape
# Skelly's release tags take.
newer_than() {
  [ "$1" != "$2" ] || return 1
  [ "$(printf '%s\n%s\n' "$1" "$2" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)" = "$1" ]
}

CHECK_ONLY=0
FORCE=0
VERSION=""
while [ $# -gt 0 ]; do
  case "$1" in
    --check) CHECK_ONLY=1 ;;
    --force) FORCE=1 ;;
    --version)
      shift
      [ $# -gt 0 ] || { echo "--version needs a release tag, e.g. --version v0.1.8" >&2; exit 2; }
      VERSION="$1"
      ;;
    --version=*) VERSION="${1#--version=}" ;;
    -h | --help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

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

# --- what is installed, and what is the latest -----------------------------------
CURRENT="${SKELLY_CURRENT_VERSION:-}"
if [ -z "$CURRENT" ] && [ -r "$RECEIPT" ]; then
  CURRENT="$(cat "$RECEIPT")"
fi
CURRENT="${CURRENT#v}"

if [ -z "$VERSION" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
  if [ -z "$VERSION" ]; then
    echo "Could not determine the latest release. Is one published yet?" >&2
    exit 1
  fi
fi
VERSION_NUM="${VERSION#v}"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"

if [ -n "$CURRENT" ] && [ "$CURRENT" = "$VERSION_NUM" ]; then
  # Nothing to do: the requested version is already the installed one.
  if [ "$CHECK_ONLY" -eq 1 ]; then
    echo "Skelly v${CURRENT} is up to date."
    exit 0
  fi
  if [ "$FORCE" -eq 0 ]; then
    echo "Skelly v${CURRENT} is already installed (use --force to reinstall)."
    exit 0
  fi
elif [ -n "$CURRENT" ] && newer_than "$CURRENT" "$VERSION_NUM"; then
  # A development build, or a hand-picked tag: report it rather than quietly downgrade.
  echo "Skelly v${CURRENT} is newer than ${VERSION}."
  if [ "$CHECK_ONLY" -eq 1 ]; then
    exit 0
  fi
  if [ "$FORCE" -eq 0 ]; then
    echo "Nothing to do (use --force to install ${VERSION} anyway)."
    exit 0
  fi
elif [ "$CHECK_ONLY" -eq 1 ]; then
  if [ -n "$CURRENT" ]; then
    echo "Update available: v${CURRENT} -> ${VERSION}"
  else
    echo "Skelly ${VERSION} is available."
  fi
  echo "Run 'skelly update' to install it."
  exit 0
fi

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
  # $1 = source binary path; place it at $BIN_DIR/skelly (sudo only if needed). It is
  # staged beside the target and renamed into place: a rename over a running executable
  # is atomic and succeeds while Skelly is open, whereas writing onto it fails (ETXTBSY).
  staged="${BIN_DIR}/.skelly.new.$$"
  if mkdir -p "$BIN_DIR" 2>/dev/null && [ -w "$BIN_DIR" ]; then
    cp "$1" "$staged"
    chmod +x "$staged"
    mv -f "$staged" "$BIN_DIR/skelly"
  else
    echo "Installing to ${BIN_DIR} (requires sudo)..."
    sudo mkdir -p "$BIN_DIR"
    sudo cp "$1" "$staged"
    sudo chmod +x "$staged"
    sudo mv -f "$staged" "$BIN_DIR/skelly"
  fi
}

record_receipt() {
  # Best-effort: the receipt only speeds up the next run's up-to-date check.
  if mkdir -p "$(dirname "$RECEIPT")" 2>/dev/null; then
    printf '%s\n' "$VERSION_NUM" > "$RECEIPT" 2>/dev/null || true
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
  record_receipt
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
  record_receipt
  echo "Installed Skelly ${VERSION} to ${BIN_DIR}/skelly"
fi

if [ -n "$CURRENT" ] && [ "$CURRENT" != "$VERSION_NUM" ]; then
  echo "Updated from v${CURRENT}. Restart any open Skelly window to pick it up."
fi

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$BIN_DIR"; then
  echo
  echo "Note: ${BIN_DIR} is not on your PATH. Add it, e.g.:"
  echo "  export PATH=\"${BIN_DIR}:\$PATH\""
fi
