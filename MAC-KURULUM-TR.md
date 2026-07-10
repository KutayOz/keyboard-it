# keyboard-it — macOS Kurulum & Paketleme

MacBook klavyesini/faresini şifreli olarak Windows PC'ye gönderen **gönderen** (sender)
tarafı. Menü çubuğunda küçük bir ikonla çalışır (Dock'ta görünmez).

---

## 1. Kullanıcı: `.dmg` ile kurulum

1. `keyboard-it-<sürüm>.dmg`'yi çift tıkla.
2. **keyboard-it**'i açılan pencerede **Applications** klasörüne sürükle.
3. İlk açılış (uygulama imzasız): uygulamayı normal aç → "Apple doğrulayamadı /
   açılmadı" uyarısını kapat → Sistem Ayarları → **Gizlilik ve Güvenlik** →
   en alttaki "keyboard-it engellendi" satırında **Yine de Aç** → uygulamayı
   tekrar aç. (macOS 14 ve öncesi: Applications'ta **sağ-tık → Aç → Aç** yeterli;
   Sequoia 15+ bu kısayolu imzasız uygulamalar için kaldırdı.)
4. **İzinler (şart):** Sistem Ayarları → **Gizlilik ve Güvenlik**:
   - **Erişilebilirlik** → keyboard-it'i ekle/işaretle
   - **Girdi İzleme** → keyboard-it'i ekle/işaretle

   İzni verdikten sonra uygulamayı bir kez kapatıp yeniden aç (yeni izin
   çalışan sürece yansımaz).
5. **Fn ayarı (şart):** Sistem Ayarları → **Klavye** → "🌐/fn tuşuna basınca" →
   **Hiçbir Şey Yapma**. Yoksa macOS çift-Fn'i Dikte/Emoji paneli için yakalar
   ve geçiş (toggle) güvenilmez çalışır.
6. Menü çubuğunda 🔒 **PASIF** ikonu belirir. **Fn'e çift bas** → 🟢 **AKTIF**;
   artık klavye/fare Windows'a gider. Tekrar Fn+Fn → PASIF. (Çift-Fn tepki
   vermiyorsa 5. adımdaki Fn ayarını kontrol et.)

### Menü çubuğu öğeleri
- **Ayarlar...** → `config.toml`'u metin editöründe açar (Windows IP'sini burada
  değiştirebilirsin; IP değişirse tek yapman gereken bu).
- **Girişte Başlat** → oturum açılışında otomatik başlatmayı aç/kapatır
  (LaunchAgent; tik = açık).
- **Cikis** → uygulamadan çıkar (imleç bağını geri verir).

### İlk ayar (config yoksa)
Uygulama Windows PC'nin IP'sini ve eşleşme anahtarını bilmeli. **Ayarlar...** ile
`config.toml`'u aç ve şu alanları doldur:

| Alan | Anlamı |
|------|--------|
| `shared_secret` | Eşleşme anahtarı — Windows tarafıyla **birebir AYNI** olmalı |
| `peer_host` | Windows PC'nin IP'si (ör. `192.168.1.105`) |
| `role` | `sender` (Mac gönderen taraf) |
| `port` | `5599` (varsayılan) |

---

## 2. Geliştirici: paketi yeniden üret

Gereksinim: Rust toolchain + Python3 (Pillow — ikon için) + macOS native araçlar
(`iconutil`, `hdiutil`, `codesign` — Xcode CLT ile gelir).

```bash
# ikon (yalnızca değiştirdiysen; .icns kaynağı repoda commit'li)
python3 packaging/mac/make_icon.py

# .app + .dmg üret -> dist/
packaging/mac/package.sh
```

Çıktı:
- `dist/keyboard-it.app` — menü-çubuğu ajanı (`LSUIElement=true`, Dock'ta yok)
- `dist/keyboard-it-<sürüm>.dmg` — sürükle-bırak kurulum imajı

### Notlar
- **Ad-hoc imza:** `codesign -s -` ile imzalanır (Apple Silicon imzasız ikiliyi
  öldürür). Apple Developer sertifikası **yok** — bu yüzden ilk açılışta Gatekeeper
  onayı gerekir (Gizlilik ve Güvenlik → "Yine de Aç"; bkz. bölüm 1, adım 3).
  Kişisel kullanım için yeterli; App Store/notarization ileride.
- **Mimari:** çıktı çalıştığın Mac'in mimarisi (Apple Silicon'da `arm64`). Universal
  (arm64+x86_64) için `x86_64-apple-darwin` hedefi kurulup `lipo` ile birleştirilebilir
  — kişisel kullanımda gerekmez.
- **Erişilebilirlik/Girdi İzleme** izni ikili YOLUNA bağlıdır: `.app`'i Applications'a
  taşıdıktan sonra izinleri yeni yol için bir kez daha vermen gerekir.

---

## Windows tarafı
Alıcı (receiver) tarafı için `WINDOWS-KURULUM-GIT-TR.md` ve `wix/` (`.msi`) dosyalarına bak.
Windows Slint tabanlı bir ayar penceresi + sistem tepsisiyle çalışır.
