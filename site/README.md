# keyboard-it — landing / indirme sayfası

Claude Design'da tasarlanan `keyboard-it.dc.html`'in bağımsız, deploy edilebilir
statik sürümü. Cloudflare Pages'e (veya herhangi bir statik host'a) olduğu gibi konur.

## İçerik
- `index.html` — tek dosya, kendine yeter (inline CSS + küçük vanilla JS scroll animasyonu).
  Claude Design'ın `support.js`/`DCLogic`'i düz JS'e, `style-hover`/`style-active`
  öznitelikleri gerçek CSS `:hover`/`:active`'e çevrildi.
- `keyboard-it.png` — favicon + hero ikonu.
- `install-macos.sh` — macOS için terminal kurulum betiği (`curl … | sh`).
  curl indirmesi quarantine bayrağı taşımadığı için Gatekeeper uyarısı hiç çıkmaz;
  DMG'yi indirir, mount eder, `.app`'i `/Applications`'a kopyalar ve açar.
  `index.html`'deki "Alternatif: Terminal ile tek komut" kutusu bu dosyaya işaret eder.
  **Deploy edilen domain değişirse** iki yer güncellenmeli: betiğin başındaki
  `BASE_URL` değişkeni ve `index.html`'deki `curl -fsSL …/install-macos.sh | sh`
  komutunun adresi.
- `downloads/` — installer'lar. **Binary'ler git'e commit EDİLMEZ** (`.gitignore`);
  deploy'dan önce buraya koyulur.

## Deploy'dan önce: installer'ları yerleştir
CI artifact'larını indir (`gh workflow run build.yml` → `gh run download <run-id> -D dist/ci`)
ve SABİT (sürümsüz) hedef adlarla kopyala — index.html bu adlara link verir,
sürüm bump'ında link kırılmaz:

```bash
cp dist/ci/macos-dmg/keyboard-it-*.dmg        site/downloads/keyboard-it-macos.dmg
cp dist/ci/windows-msi/keyboard-it-*-x64.msi  site/downloads/keyboard-it-windows-x64.msi
```

(CI, cargo-wix çıktısını paketlemeden sonra `keyboard-it-<sürüm>-x64.msi` adına
çevirir — bkz. `.github/workflows/build.yml` "ürün adına çevir" adımı.)

## Sürüm bump kontrol listesi
İndirme linkleri sürümsüz olduğu için değişmez; `index.html`'de yalnızca iki nokta
elle güncellenir:
1. `<legend>` içindeki "Download — vX.Y.Z" sürümü,
2. buton altındaki dosya boyutları (`ls -lh site/downloads/` ile bak).

Eski sürümün dosyaları kopyalamada üzerine yazıldığı için `downloads/`'ta bayat
dosya kalmaz.

## Cloudflare Pages'e yükleme
- **Dashboard (en basit):** Cloudflare Pages → "Upload assets" → tüm `site/` klasörünü
  sürükle. `<proje>.pages.dev` adresinde yayında.
- **Wrangler (CLI):** `npx wrangler pages deploy site`
- İndirme butonları `downloads/…` göreli yolunu kullanır; klasörle birlikte gittiği
  için ekstra ayar gerekmez. İki dosya da <25 MB (Pages limiti) ✓.

## Yerel önizleme
```bash
python3 -m http.server 8099 --directory site
# http://localhost:8099
```
