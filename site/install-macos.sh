#!/bin/sh
# keyboard-it — macOS terminal install
# Usage: curl -fsSL https://kutayoz.github.io/keyboard-it/install-macos.sh | sh
# Pin image and checksum manifest to the same release to avoid latest-link races.

set -eu

# Manual download page (shown in error messages).
BASE_URL="https://kutayoz.github.io/keyboard-it"

VERSION="0.6.2"
RELEASE_URL="https://github.com/KutayOz/keyboard-it/releases/download/v$VERSION"
DMG_URL="$RELEASE_URL/keyboard-it-macos.dmg"
APP_NAME="keyboard-it.app"
DEST_DIR="/Applications"
# A private, unpredictable directory prevents other local users from planting
# symlinks at a predictable download or mount path.
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/keyboard-it.XXXXXXXX")
TMP_DMG="$WORK_DIR/keyboard-it-macos.dmg"
CHECKSUMS="$WORK_DIR/SHA256SUMS"
MNT="$WORK_DIR/mnt"
MOUNTED=0

fail() {
    echo "" >&2
    echo "ERROR: $1" >&2
    exit 1
}

# On exit (success or failure) detach the DMG and remove the temp file.
cleanup() {
    if [ "$MOUNTED" = 1 ]; then
        hdiutil detach "$MNT" -quiet 2>/dev/null || true
    fi
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

echo "keyboard-it macOS install"
echo "========================="
echo "This script downloads the DMG, mounts it, copies the app into"
echo "$DEST_DIR and launches it after checking the release SHA-256 digest."
echo "The app is unsigned; checksums do not verify a publisher's identity."
echo ""

echo "[1/5] Downloading: $DMG_URL"
curl -fSL -o "$TMP_DMG" "$DMG_URL" \
    || fail "Download failed. Check your internet connection; if the problem persists, download manually from $BASE_URL."

echo "[2/5] Verifying the release SHA-256 checksum..."
curl -fSL -o "$CHECKSUMS" "$RELEASE_URL/SHA256SUMS" \
    || fail "Could not download the release checksum manifest. Nothing was installed."
EXPECTED=$(awk '$2 == "keyboard-it-macos.dmg" { print $1 }' "$CHECKSUMS")
[ "${#EXPECTED}" -eq 64 ] || fail "Invalid or missing DMG checksum. Nothing was installed."
case "$EXPECTED" in *[!0-9a-fA-F]*) fail "Invalid DMG checksum. Nothing was installed." ;; esac
ACTUAL=$(shasum -a 256 "$TMP_DMG" | awk '{ print $1 }')
[ "$ACTUAL" = "$EXPECTED" ] || fail "DMG checksum mismatch. Nothing was installed."

mkdir "$MNT"
echo "[3/5] Mounting the disk image (hdiutil attach)..."
hdiutil attach "$TMP_DMG" -nobrowse -readonly -quiet -mountpoint "$MNT" \
    || fail "Could not mount the DMG. The file may be corrupt; run the script again."

MOUNTED=1
APP_SRC="$MNT/$APP_NAME"
[ -d "$APP_SRC" ] || fail "$APP_NAME not found inside the DMG. The downloaded file is not the expected installer."

echo "[4/5] Copying $APP_NAME into $DEST_DIR (replaces any previous version)..."
rm -rf "$DEST_DIR/$APP_NAME" 2>/dev/null || true
ditto "$APP_SRC" "$DEST_DIR/$APP_NAME" \
    || fail "Could not copy into $DEST_DIR. Try again as an admin user."

echo "[5/5] Launching the app..."
open "$DEST_DIR/$APP_NAME" \
    || fail "Could not launch the app. You can open keyboard-it manually from $DEST_DIR."

echo ""
echo "Install complete. keyboard-it will appear in the menu bar (top right)."
echo "On first launch, grant the Accessibility and Input Monitoring permissions"
echo "that macOS asks for."
