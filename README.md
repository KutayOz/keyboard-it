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

- **M0 ✅:** win-receiver Windows'ta SendInput ile tuş enjekte eder. (Windows'ta doğrulandı.)
- **M1 ✅ (kod):** tuşlar Mac -> Windows, TCP üzerinden. Mac'te dry-run ile doğrulandı;
  iki-makine testi için bkz. `M1-HANDOFF-TR.md`.
- **M2:** gerçek CGEventTap yakalama (mac-sender), `src/capture.rs`'i devreye al.
- **M3:** çift-tıklama-Fn toggle.

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
