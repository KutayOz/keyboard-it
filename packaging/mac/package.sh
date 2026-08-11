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

# Privacy keys: without NSLocalNetworkUsageDescription macOS 15+ raises the Local
# Network prompt with no explanation of what is asking or why. NSBonjourServices takes
# the service type WITHOUT the .local. suffix that protocol::MDNS_SERVICE carries.
# (Caveat: mdns-sd opens its own multicast socket rather than going through
# DNSServiceBrowse, so the key may not be what gates it — macOS enforces Local Network
# at the socket layer either way. It is correct, required for the NSNetService path,
# and harmless.) NSAccessibilityUsageDescription is deliberately absent:
# AXIsProcessTrustedWithOptions draws fixed system copy and ignores it.
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
	<key>NSLocalNetworkUsageDescription</key>
	<string>keyboard-it finds your Windows PC on this network so you can pair with it without typing an IP address.</string>
	<key>NSBonjourServices</key>
	<array>
		<string>_keyboard-it._tcp</string>
	</array>
	<key>NSInputMonitoringUsageDescription</key>
	<string>keyboard-it reads your keystrokes so it can forward them to your Windows PC.</string>
</dict>
</plist>
PLIST

# Signing. Apple Silicon kills unsigned binaries, so the bundle is always signed —
# but WITH WHAT decides whether the user has to re-grant permissions on every update.
#
# An ad-hoc signature has no certificate, so codesign falls back to a designated
# requirement of `cdhash H"..."` — the permission is bound to this exact binary.
# Rebuild and the hash changes, the System Settings row stays, and macOS refuses the
# app while its switch still reads ON. Worse, one saved answer per bundle id means it
# never prompts again; the only cure is deleting the row (or `sudo tccutil reset`).
#
# Signing with a certificate instead gives a requirement of `identifier "..." and
# certificate leaf = H"<cert>"`, which survives every rebuild. A self-signed cert is
# enough — TCC does not care whether Apple issued it:
#
#   Keychain Access > Certificate Assistant > Create a Certificate…
#     name: keyboard-it,  type: Code Signing,  self-signed
#   then: export KEYBOARD_IT_SIGN_ID="keyboard-it"
SIGN_ID="${KEYBOARD_IT_SIGN_ID:-}"
if [ -n "$SIGN_ID" ]; then
  echo "==> signing with identity: $SIGN_ID"
  codesign --force -s "$SIGN_ID" "$APP" ||
    { echo "   signing FAILED — check 'security find-identity -v -p codesigning'"; exit 1; }
else
  # Ad-hoc, but with an EXPLICIT designated requirement. Left to itself codesign
  # gives an ad-hoc bundle a requirement of `cdhash H"..."`, which is why every
  # rebuild used to lose Input Monitoring and Accessibility: TCC stores the
  # requirement at grant time, the next build no longer satisfies it, and macOS
  # then refuses the app AND declines to prompt again, because it still has one
  # saved answer for this bundle identifier.
  #
  # Pinning the requirement to the identifier alone makes the grant outlive the
  # build. The trade-off is real and worth stating: any binary claiming this
  # bundle identifier would inherit the permission. For a personal LAN tool that
  # is a better bargain than sending the user to `sudo tccutil reset` after every
  # update — and KEYBOARD_IT_SIGN_ID above is the strictly better answer, since a
  # certificate-based requirement is both stable AND scoped to that certificate.
  echo "==> ad-hoc signing with identifier-pinned requirement"
  # NOT "|| true". A half-signed bundle is killed on sight by Apple Silicon, and the
  # failure this used to swallow was a running copy of the app holding the bundle
  # busy — which produced an .app with no _CodeSignature at all and a build that
  # looked successful.
  codesign --force -s - -r="designated => identifier \"$BUNDLE_ID\"" "$APP" ||
    { echo "   signing FAILED — quit any running copy of keyboard-it and re-run"; exit 1; }
fi

# The signature is the thing macOS gates launching AND every permission on, so it is
# checked here rather than discovered as a mysterious failure to start.
codesign -v "$APP" || { echo "   signature does not verify — refusing to ship it"; exit 1; }
echo "   requirement: $(codesign -d -r- "$APP" 2>/dev/null | tail -1)"

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
echo "Permissions: the app opens a Permissions window on first launch and walks through"
echo "             Input Monitoring, Accessibility and Local Network in order, then"
echo "             restarts once. Reopen it any time from the menu bar."
