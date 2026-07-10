#!/bin/sh
# keyboard-it — macOS terminal install (quarantine-free path)
#
# Files downloaded with curl do not get the macOS quarantine flag, so Gatekeeper's
# "unidentified developer" warning never appears.
# Usage:  curl -fsSL https://kutayoz.github.io/keyboard-it/install-macos.sh | sh
#
# NOTE: the DMG comes from the version-independent 'latest' link on GitHub Releases;
# the script keeps working even if the site domain changes (see site/README.md).

set -eu

# Manual download page (shown in error messages).
BASE_URL="https://kutayoz.github.io/keyboard-it"

DMG_URL="https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-macos.dmg"
APP_NAME="keyboard-it.app"
DEST_DIR="/Applications"
TMP_DMG="/tmp/keyboard-it-macos.$$.dmg"
MNT="/tmp/keyboard-it-mnt.$$"

fail() {
    echo "" >&2
    echo "ERROR: $1" >&2
    exit 1
}

# On exit (success or failure) detach the DMG and remove the temp file.
cleanup() {
    if [ -d "$MNT" ]; then
        hdiutil detach "$MNT" -quiet 2>/dev/null || true
    fi
    rm -f "$TMP_DMG"
}
trap cleanup EXIT

echo "keyboard-it macOS install"
echo "========================="
echo "This script downloads the DMG, mounts it, copies the app into"
echo "$DEST_DIR and launches it. Because the download happens in the"
echo "terminal, no macOS Gatekeeper warning appears."
echo ""

echo "[1/4] Downloading: $DMG_URL"
curl -fSL -o "$TMP_DMG" "$DMG_URL" \
    || fail "Download failed. Check your internet connection; if the problem persists, download manually from $BASE_URL."

echo "[2/4] Mounting the disk image (hdiutil attach)..."
hdiutil attach "$TMP_DMG" -nobrowse -readonly -quiet -mountpoint "$MNT" \
    || fail "Could not mount the DMG. The file may be corrupt; run the script again."

APP_SRC="$MNT/$APP_NAME"
[ -d "$APP_SRC" ] || fail "$APP_NAME not found inside the DMG. The downloaded file is not the expected installer."

echo "[3/4] Copying $APP_NAME into $DEST_DIR (replaces any previous version)..."
rm -rf "$DEST_DIR/$APP_NAME" 2>/dev/null || true
ditto "$APP_SRC" "$DEST_DIR/$APP_NAME" \
    || fail "Could not copy into $DEST_DIR. Try again as an admin user."

echo "[4/4] Launching the app..."
open "$DEST_DIR/$APP_NAME" \
    || fail "Could not launch the app. You can open keyboard-it manually from $DEST_DIR."

echo ""
echo "Install complete. keyboard-it will appear in the menu bar (top right)."
echo "On first launch, grant the Accessibility and Input Monitoring permissions"
echo "that macOS asks for."
