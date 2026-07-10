#!/bin/sh
# keyboard-it — macOS terminal kurulumu (quarantine'siz yol)
#
# curl ile indirilen dosyalara macOS quarantine bayrağı eklenmez; bu yüzden
# Gatekeeper'ın "tanımlanamayan geliştirici" uyarısı hiç çıkmaz.
# Kullanım:  curl -fsSL https://kutayoz.github.io/keyboard-it/install-macos.sh | sh
#
# NOT: DMG, GitHub Releases'in sürümden bağımsız 'latest' linkinden iner;
# site domain'i değişse bile bu betiğin çalışması bozulmaz (bkz. site/README.md).

set -eu

# Elle indirme sayfası (hata mesajlarında gösterilir).
BASE_URL="https://kutayoz.github.io/keyboard-it"

DMG_URL="https://github.com/KutayOz/keyboard-it/releases/latest/download/keyboard-it-macos.dmg"
APP_NAME="keyboard-it.app"
DEST_DIR="/Applications"
TMP_DMG="/tmp/keyboard-it-macos.$$.dmg"
MNT="/tmp/keyboard-it-mnt.$$"

hata() {
    echo "" >&2
    echo "HATA: $1" >&2
    exit 1
}

# Çıkışta (başarı ya da hata) DMG'yi ayır ve geçici dosyayı sil.
temizlik() {
    if [ -d "$MNT" ]; then
        hdiutil detach "$MNT" -quiet 2>/dev/null || true
    fi
    rm -f "$TMP_DMG"
}
trap temizlik EXIT

echo "keyboard-it macOS kurulumu"
echo "=========================="
echo "Bu betik sirasiyla: DMG'yi indirir, baglar (mount), uygulamayi"
echo "$DEST_DIR icine kopyalar ve calistirir. Terminal ile indirildigi"
echo "icin macOS Gatekeeper uyarisi cikmaz."
echo ""

echo "[1/4] Indiriliyor: $DMG_URL"
curl -fSL -o "$TMP_DMG" "$DMG_URL" \
    || hata "Indirme basarisiz oldu. Internet baglantinizi kontrol edin; sorun surerse $BASE_URL adresinden elle indirin."

echo "[2/4] Disk goruntusu baglaniyor (hdiutil attach)..."
hdiutil attach "$TMP_DMG" -nobrowse -readonly -quiet -mountpoint "$MNT" \
    || hata "DMG baglanamadi. Dosya bozuk inmis olabilir; betigi yeniden calistirin."

APP_SRC="$MNT/$APP_NAME"
[ -d "$APP_SRC" ] || hata "DMG icinde $APP_NAME bulunamadi. Indirilen dosya beklenen kurulum paketi degil."

echo "[3/4] $APP_NAME, $DEST_DIR icine kopyalaniyor (varsa eski surumun ustune yazilir)..."
rm -rf "$DEST_DIR/$APP_NAME" 2>/dev/null || true
ditto "$APP_SRC" "$DEST_DIR/$APP_NAME" \
    || hata "$DEST_DIR icine kopyalanamadi. Yonetici (admin) bir kullanici ile deneyin."

echo "[4/4] Uygulama aciliyor..."
open "$DEST_DIR/$APP_NAME" \
    || hata "Uygulama acilamadi. $DEST_DIR icinden keyboard-it'i elle acabilirsiniz."

echo ""
echo "Kurulum tamamlandi. keyboard-it menu cubugunda (sag ust) gorunecek."
echo "Ilk aciliste macOS'in isteyecegi Erisilebilirlik ve Giris Izleme"
echo "(Accessibility / Input Monitoring) izinlerini vermeyi unutmayin."
