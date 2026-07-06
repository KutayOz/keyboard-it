# Windows Oto-Başlatma (win-receiver) — Handoff

> **Amaç:** `win-receiver`, Windows oturumu açıldığında **arka planda otomatik** başlasın;
> her seferinde terminalden çalıştırmaya gerek kalmasın. Mac bağlanınca klavye+fare hazır olur.

## Neden Servis DEĞİL, "oturum açılışı görevi"?

Tuş/fare enjeksiyonu (`SendInput`) **kullanıcının interaktif oturumunda** çalışmak zorundadır.
Windows Service (session 0) masaüstüne giriş enjekte **edemez**. Bu yüzden doğru yol
**Task Scheduler "at logon"** görevidir ("Run only when user is logged on").

---

## Ön koşullar

1. Kod güncel + **release** derlenmiş olmalı:
   ```powershell
   cd C:\yol\keyboard-it
   git pull
   cargo build --release -p win-receiver
   ```
   Sonuç: `target\release\win-receiver.exe`.
2. `KEYBOARD_IT_KEY` kalıcı ayarlı olmalı (daha önce `setx` ile yapıldı). Oturum açılışı
   görevi kullanıcı ortam değişkenlerini miras alır — kontrol: yeni bir terminalde
   `echo %KEYBOARD_IT_KEY%` değeri yazmalı. Boşsa: `setx KEYBOARD_IT_KEY "AYNI-PAROLA"` + yeni terminal.

---

## Kurulum — iki seçenek

### Seçenek A (önerilen): GİZLİ başlat (konsol penceresi açılmaz)

Repodaki `windows\start-hidden.vbs` exe'yi gizli başlatır (yolu kendi konumuna göre çözer).
`C:\TAM\YOL` kısmını gerçek yolla değiştir:

```powershell
schtasks /create /tn "keyboard-it" /sc onlogon /f ^
  /tr "wscript.exe \"C:\TAM\YOL\keyboard-it\windows\start-hidden.vbs\""
```

### Seçenek B: görünür konsol (basit, ama açılışta pencere belirir)

```powershell
schtasks /create /tn "keyboard-it" /sc onlogon /f ^
  /tr "\"C:\TAM\YOL\keyboard-it\target\release\win-receiver.exe\""
```

> Yükseltilmiş (admin) pencerelere de kontrol istiyorsan komuta `/rl highest` ekle
> (o zaman oturum açılışında elevated başlar; UAC'siz).

---

## Test

Yeniden başlatmadan hemen dene:
```powershell
schtasks /run /tn "keyboard-it"
```
- Görev Yöneticisi > Ayrıntılar'da **win-receiver.exe** görünmeli (Seçenek A'da pencere olmadan).
- Mac'ten bağlanınca (mac-sender), win-receiver çalışıyorsa bağlantı kurulmalı. Seçenek A'da
  çıktı gizli olduğundan görmezsin; bağlantının kurulduğunu Mac tarafındaki
  "şifreli kanal kuruldu" + gerçek tuş/fare kontrolüyle doğrula.

Çalıştığını gördükten sonra: bir sonraki **oturum açılışında** otomatik başlar.

---

## Yönetim

- **Durdur (bu oturumda):** Görev Yöneticisi > Ayrıntılar > `win-receiver.exe` > Görevi sonlandır.
- **Görevi kaldır:** `schtasks /delete /tn "keyboard-it" /f`
- **Görevi gör:** `schtasks /query /tn "keyboard-it" /v /fo list`

---

## Sorun giderme

| Belirti | Sebep | Çözüm |
|---|---|---|
| Görev çalışıyor ama Mac bağlanamıyor | `KEYBOARD_IT_KEY` görev ortamında yok/yanlış | Yeni terminalde `echo %KEYBOARD_IT_KEY%` kontrol; `setx` + yeniden oturum aç (görev yeni ortamı bir sonraki açılışta alır). |
| VBS "hiçbir şey yapmadı" | `win-receiver.exe` yok (release derlenmemiş) | `cargo build --release -p win-receiver`. |
| Açılışta kısa siyah pencere yanıp söndü | Seçenek B (görünür) kullanıldı | Seçenek A'ya (VBS gizli) geç: görevi silip A ile yeniden oluştur. |
| Güvenlik duvarı sordu | İlk gelen bağlantı | Private ağ için izin ver (daha önce yapıldıysa tekrar sormaz). |
| Enjeksiyon yükseltilmiş pencerede çalışmıyor | UIPI | Görevi `/rl highest` ile yeniden oluştur (elevated başlar). |

---

## Rapor et

- Hangi seçenek (A/B), görev oluştu mu (`schtasks /query` çıktısı).
- `schtasks /run` sonrası win-receiver.exe Görev Yöneticisi'nde göründü mü.
- Mac'ten bağlanınca kontrol çalıştı mı.
- Hata varsa tam metin.
