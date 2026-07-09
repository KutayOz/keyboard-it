# keyboard-it — landing / indirme sayfası

Claude Design'da tasarlanan `keyboard-it.dc.html`'in bağımsız, deploy edilebilir
statik sürümü. Cloudflare Pages'e (veya herhangi bir statik host'a) olduğu gibi konur.

## İçerik
- `index.html` — tek dosya, kendine yeter (inline CSS + küçük vanilla JS scroll animasyonu).
  Claude Design'ın `support.js`/`DCLogic`'i düz JS'e, `style-hover`/`style-active`
  öznitelikleri gerçek CSS `:hover`/`:active`'e çevrildi.
- `keyboard-it.png` — favicon + hero ikonu.
- `downloads/` — installer'lar. **Binary'ler git'e commit EDİLMEZ** (`.gitignore`);
  deploy'dan önce buraya koyulur.

## Deploy'dan önce: installer'ları yerleştir
CI'dan gelen güncel dosyaları buraya kopyala:

```bash
cp dist/upload/keyboard-it-0.1.0.dmg      site/downloads/
cp dist/upload/keyboard-it-0.1.0-x64.msi  site/downloads/
```

(Yeni sürüm için: `gh workflow run build.yml` → artifact'ları indir → `dist/upload`'a al.)

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
