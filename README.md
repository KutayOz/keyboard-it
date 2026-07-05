# keyboard-it

MacBook Air'ın dahili klavyesiyle bir Windows PC'yi kontrol etmek. Aç/kapa: **Fn'e çift bas**.

Yaklaşım: iki bilgisayara da kendi yazılımını kur, LAN üzerinden konuşsunlar
(klavyeye indirgenmiş kendi Synergy/Deskflow'un). Donanım yok, Bluetooth yok.

```
Mac: dahili klavye -> CGEventTap (yakala + Fn toggle) -> HID usage'a çevir
     -> framed TCP (ileride TLS) --LAN--> Windows: al -> HID->scancode
     -> SendInput(KEYEVENTF_SCANCODE) -> odaktaki uygulama
```

## Workspace

- `crates/protocol` — paylaşılan tel formatı (`KeyEvent` + encode/decode). OS'tan bağımsız, iki tarafın ortak dili.
- `crates/mac-sender` — macOS binary (CGEventTap yakalama). Gerçek kod M2'de; `src/capture.rs` referans.
- `crates/win-receiver` — Windows binary (`SendInput` enjeksiyon). M0 hazır.

## Kilometre taşları

- **M0 ✅:** win-receiver Windows'ta SendInput ile tuş enjekte eder. (Doğrulandı.)
- **M1 ✅:** tuşlar Mac -> Windows, TCP üzerinden. (İki makinede doğrulandı.)
- **M2 ✅:** gerçek CGEventTap yakalama — Mac'te yaz, Windows'ta çık. (Doğrulandı.)
- **M3 ✅:** çift-tıklama-Fn ile aç/kapa + aktifken Mac'te bastırma.
- **Cila ✅:** Cmd→Ctrl + Türkçe Q + F-tuşları; TLS-benzeri Noise şifreleme; otomatik
  yeniden bağlanma; kopmada takılı tuş bırakma; IP'yi hatırlama.
- **Fare ✅:** trackpad/fare hareketi + tıklama + scroll (AKTİF iken Windows'a; Mac imleci
  donar = KVM). Tek kaçış AKTİF iken çift-Fn.

## Şifreleme (zorunlu)

Trafik Noise (`NNpsk0`) ile şifreli + karşılıklı doğrulanır. **İki makinede de AYNI
`KEYBOARD_IT_KEY` parolası** ayarlı olmalı; yoksa program açılmaz, yanlışsa bağlantı reddedilir.

```sh
# Ortak, güçlü bir parola üret (bir kez), İKİ makinede de aynısını kullan:
openssl rand -base64 24
# Mac:      export KEYBOARD_IT_KEY='ürettiğin-değer'   (~/.zshrc'ye ekle)
# Windows:  setx KEYBOARD_IT_KEY "ürettiğin-değer"     (yeni terminal aç)
```

## Çalıştır (asıl özellik)

**Ön koşul (Mac):** Sistem Ayarları > Klavye > "🌐/fn tuşuna basınca → Hiçbir şey yapma".
**İzin (Mac):** terminale hem Giriş İzleme hem Erişilebilirlik ver.
**Anahtar:** iki tarafta da `KEYBOARD_IT_KEY` ayarlı (yukarı bkz).

```sh
# Windows'ta: git pull + cargo run -p win-receiver, Notepad odakta.
# Mac'te:
cargo run -p mac-sender -- <windows-ip>
# PASİF başlar. Fn'e çift bas → AKTİF (yazdığın Windows'a gider, Mac'te bastırılır).
# Tekrar çift bas → PASİF. Kilitlenirsen: fareyle  menü > Force Quit.
```

## M1'i çalıştır (iki makine)

**Windows PC'de** (dinleyici — Notepad odakta tut, terminal değil):
```sh
cargo run -p win-receiver     # 0.0.0.0:5599 dinler; güvenlik duvarına izin ver
```

**Mac'te** (gönderici — <windows-ip> = Windows'un LAN IP'si):
```sh
cargo run -p mac-sender -- <windows-ip>
# Windows'ta Notepad'de "hello" belirmeli
```

Ayrıntılı adım-adım + sorun giderme: `M1-HANDOFF-TR.md`.

## Yerel dry-run testi (yalnız Mac, Windows olmadan)

win-receiver Windows dışında enjekte etmez, gelen tuşları yazdırır — tüm ağ yolunu
Mac'te test etmeni sağlar:
```sh
cargo run -p win-receiver          # bir terminalde (dry-run dinleyici)
cargo run -p mac-sender -- 127.0.0.1   # başka terminalde
```
