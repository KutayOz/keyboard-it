#!/usr/bin/env bash
# keyboard-it — macOS dağıtım paketleyici.
# release derler -> keyboard-it.app (menü-çubuğu ajanı) -> keyboard-it-<sürüm>.dmg
# Native araçlar: cargo + codesign + hdiutil. Ekstra bağımlılık yok.
#
# Kullanım:  packaging/mac/package.sh
# Çıktı:     dist/keyboard-it.app  ve  dist/keyboard-it-<sürüm>.dmg
set -euo pipefail

APP_NAME="keyboard-it"
BUNDLE_ID="com.keyboard-it.keyboard-it"
DISPLAY_NAME="keyboard-it"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Sürümü kök Cargo.toml'daki [workspace.package] bölümünden çek
# (crate'ler version.workspace = true ile buradan miras alıyor).
VERSION="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -z "$VERSION" ] || [[ "$VERSION" == *=* ]]; then
  echo "HATA: kök Cargo.toml'dan sürüm okunamadı" >&2
  exit 1
fi
DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
ICNS="$ROOT/crates/mac-sender/assets/$APP_NAME.icns"
BIN="$ROOT/target/release/mac-sender"

if [ ! -f "$ICNS" ]; then
  echo "==> ikon bulunamadı, üretiliyor"
  python3 "$ROOT/packaging/mac/make_icon.py"
fi

echo "==> release derleniyor (opt-level=z, lto) — biraz sürebilir"
cargo build --release -p mac-sender

echo "==> $APP_NAME.app iskeleti kuruluyor"
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
	<string>keyboard-it — kişisel LAN klavye/fare köprüsü</string>
</dict>
</plist>
PLIST

# Ad-hoc imza: Apple Silicon'da imzasız ikili öldürülür; bundle'ı yeniden imzala.
echo "==> ad-hoc imzalanıyor (codesign -s -)"
codesign --force -s - "$APP" 2>/dev/null || echo "   (codesign atlandı — sorun değil)"

echo "==> .dmg oluşturuluyor"
DMG="$DIST/$APP_NAME-$VERSION.dmg"
rm -f "$DMG"
STAGING="$(mktemp -d)"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGING"

echo ""
echo "✅ hazır:"
echo "   .app : $APP"
echo "   .dmg : $DMG"
echo ""
echo "Kurulum: .dmg'yi aç, keyboard-it'i Applications'a sürükle."
echo "İlk açılış (imzasız): Applications'ta sağ-tık -> Aç -> Aç."
echo "İzin: Sistem Ayarları -> Gizlilik & Güvenlik -> Erişilebilirlik + Girdi İzleme'de"
echo "      keyboard-it'i işaretle (yakalama için şart)."
