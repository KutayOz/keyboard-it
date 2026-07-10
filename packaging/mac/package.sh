#!/usr/bin/env bash
# keyboard-it — macOS distribution packager.
# Builds release -> keyboard-it.app (menu-bar agent) -> keyboard-it-<version>.dmg
# Tools: cargo + codesign + dmgbuild (pip; auto-installed if missing).
#
# Usage:   packaging/mac/package.sh
# Output:  dist/keyboard-it.app  and  dist/keyboard-it-<version>.dmg
set -euo pipefail

APP_NAME="keyboard-it"
BUNDLE_ID="com.keyboard-it.keyboard-it"
DISPLAY_NAME="keyboard-it"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Read the version from [workspace.package] in the root Cargo.toml
# (the crates inherit it via version.workspace = true).
VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -z "$VERSION" ] || [[ "$VERSION" == *=* ]]; then
  echo "ERROR: could not read version from root Cargo.toml" >&2
  exit 1
fi
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
ICNS="$ROOT/crates/mac-sender/assets/$APP_NAME.icns"
BIN="$ROOT/target/release/mac-sender"

if [ ! -f "$ICNS" ]; then
  echo "==> icon not found, generating"
  python3 "$ROOT/packaging/mac/make_icon.py"
fi

echo "==> building release (opt-level=z, lto) — this can take a while"
cargo build --release -p mac-sender

echo "==> assembling $APP_NAME.app skeleton"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"
chmod +x "$APP/Contents/MacOS/$APP_NAME"
cp "$ICNS" "$APP/Contents/Resources/$APP_NAME.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>$DISPLAY_NAME</string>
	<key>CFBundleDisplayName</key>
	<string>$DISPLAY_NAME</string>
	<key>CFBundleIdentifier</key>
	<string>$BUNDLE_ID</string>
	<key>CFBundleExecutable</key>
	<string>$APP_NAME</string>
	<key>CFBundleIconFile</key>
	<string>$APP_NAME</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$VERSION</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>keyboard-it — personal LAN keyboard/mouse bridge</string>
</dict>
</plist>
PLIST

# Ad-hoc signature: Apple Silicon kills unsigned binaries; re-sign the bundle.
echo "==> ad-hoc signing (codesign -s -)"
codesign --force -s - "$APP" 2>/dev/null || echo "   (codesign skipped — not fatal)"

# Branded installer window (background + fixed icon layout) via dmgbuild.
# The retina background TIFF is committed; regenerate with make_dmg_background.py.
if ! python3 -c "import dmgbuild" >/dev/null 2>&1; then
  echo "==> installing dmgbuild (pip --user)"
  # Homebrew Python is PEP 668 "externally managed"; --user still needs the override.
  python3 -m pip install --quiet --user dmgbuild 2>/dev/null ||
    python3 -m pip install --quiet --user --break-system-packages dmgbuild
fi

echo "==> creating .dmg (dmgbuild)"
DMG="$DIST/$APP_NAME-$VERSION.dmg"
rm -f "$DMG"
python3 -m dmgbuild -s "$ROOT/packaging/mac/dmg-settings.py" \
  -D app="$APP" -D settings_dir="$ROOT/packaging/mac" "$APP_NAME" "$DMG"

echo ""
echo "done:"
echo "   .app : $APP"
echo "   .dmg : $DMG"
echo ""
echo "Install: open the .dmg and drag keyboard-it into Applications."
echo "First launch (unsigned): in Applications, right-click -> Open -> Open."
echo "Permissions: System Settings -> Privacy & Security -> enable keyboard-it under"
echo "             Accessibility and Input Monitoring (required for capture)."
