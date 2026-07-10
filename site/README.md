# keyboard-it — landing / indirme sayfası

Claude Design'da tasarlanan `keyboard-it.dc.html`'in bağımsız, deploy edilebilir
statik sürümü.

## Dağıtım mimarisi
- **Birincil site: GitHub Pages** → `https://kutayoz.github.io/keyboard-it/`
  `main`'e `site/**` push'unda `.github/workflows/pages.yml` otomatik deploy eder.
  Neden: `*.pages.dev` Türkiye'de ISS seviyesinde SNI filtresine takılıyor
  (TLS Client Hello'da RST); `github.io` erişilebilir.
- **Yedek/ayna: Cloudflare Pages** → `https://keyboard-it.pages.dev`
  Elle güncellenir: `npx wrangler pages deploy site --project-name keyboard-it`
- **Installer'lar: GitHub Releases.** İndirme butonları ve `install-macos.sh`,
  sürümden bağımsız `releases/latest/download/<sabit-ad>` linklerini kullanır:
  - `keyboard-it-macos.dmg`
  - `keyboard-it-windows-x64.msi`
  Bu sabit adlı kopyaları her `v*` tag'inde CI'ın release job'ı üretir
  (bkz. `.github/workflows/build.yml` "Sabit adlı kopyalar" adımı); yanlarında
  sürümlü adlar da (`keyboard-it-0.1.0.dmg` vb.) arşiv amaçlı durur.

## İçerik
- `index.html` — tek dosya, kendine yeter (inline CSS + küçük vanilla JS scroll
  animasyonu). İndirme linkleri GitHub Releases'e gider; sitede binary taşınmaz.
- `keyboard-it.png` — favicon + hero ikonu.
- `install-macos.sh` — macOS için terminal kurulum betiği (`curl … | sh`).
  curl indirmesi quarantine bayrağı taşımadığı için Gatekeeper uyarısı hiç çıkmaz;
  DMG'yi GitHub Releases'ten indirir, mount eder, `.app`'i `/Applications`'a
  kopyalar ve açar. `index.html`'deki "Alternatif: Terminal ile tek komut" kutusu
  bu dosyaya işaret eder. **Site domain'i değişirse** betiğin başındaki `BASE_URL`
  (yalnız hata mesajlarında görünür) ve `index.html`'deki `curl …/install-macos.sh`
  adresi güncellenir; DMG linki Releases'te olduğu için kurulum yine çalışır.
- `downloads/` — repoda boş durur (`.gitkeep`). Yerelde test için CI çıktıları
  buraya konabilir; binary'ler git'e commit EDİLMEZ (`.gitignore`).

## Yeni sürüm çıkarma
1. Sürümü kök `Cargo.toml`'daki `[workspace.package]`'ta yükselt, commit'le.
2. `git tag vX.Y.Z && git push origin vX.Y.Z` — CI derler, Release oluşturur,
   sabit adlı kopyaları ekler. Site linkleri kendiliğinden yeni sürümü gösterir.
3. `index.html`'de elle güncellenecek iki nokta: `<legend>` içindeki
   "Download — vX.Y.Z" ve buton altındaki dosya boyutları.
4. Site değişikliğini push'la (GitHub Pages otomatik); istersen Cloudflare
   aynasını da wrangler ile tazele.

## Yerel önizleme
```bash
python3 -m http.server 8099 --directory site
# http://localhost:8099
```
