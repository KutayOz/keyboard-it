# keyboard-it — M1 Handoff (Windows Ajanı İçin)

> **Sana görev:** M0 geçti (Windows'a tuş enjeksiyonu kanıtlandı). Şimdi **M1**: Mac'ten
> **TCP üzerinden** gelen tuşları bu Windows PC'ye enjekte et. Yani artık `win-receiver`
> önyüklemede "hello" yazmıyor; bir **TCP portu dinliyor** ve Mac'ten gelen tuşları basıyor.
> Bu, iki-makine bir testtir: sen Windows'u sürüyorsun, kullanıcı Mac'i.

---

## 1. Ne değişti (M0 → M1)

- `win-receiver` artık bir **TCP sunucusudur**: `0.0.0.0:5599` dinler, gelen `KeyEvent`
  mesajlarını çözer, HID→scancode çevirir ve `SendInput` ile basar.
- `mac-sender` artık bir **TCP istemcisidir**: Windows'un IP'sine bağlanır ve "hello"yu
  tuş olayları olarak gönderir. (Mac'i kullanıcı çalıştıracak — senin işin değil.)
- **Toolchain zaten kurulu** (M0'da yaptın). Yeni kurulum yok.

> **ÖNEMLİ — kodu tazele (git ile):** Proje artık GitHub'da. Klasörü elle kopyalama —
> `git pull` ile güncelle:
> ```powershell
> cd C:\yol\keyboard-it
> git pull
> ```
> İlk kez alıyorsan repoyu clone et (özel repo, bir kereye mahsus GitHub girişi gerekir) —
> bkz. **`WINDOWS-KURULUM-GIT-TR.md`**. Kod güncelse `crates/win-receiver/src/` altında
> `inject.rs` ve `scancode.rs` dosyaları vardır.

---

## 2. Bu PC'nin LAN IP'sini bul (kullanıcıya bildir)

Mac'in bu makineye bağlanabilmesi için IP gerekli:

```powershell
ipconfig
```

`IPv4 Address` satırındaki adresi al (ör. `192.168.1.42`). **Bu IP'yi kullanıcıya
bildir** — Mac tarafında `mac-sender` bununla çağrılacak.

> İki makine de **aynı ağda** olmalı (aynı Wi-Fi/switch). Windows ağ profili **Private**
> olmalı (Public'te güvenlik duvarı daha katıdır).

---

## 3. Güvenlik duvarı (gelen bağlantıya izin)

`win-receiver` gelen bir TCP bağlantısı kabul eder; Windows Defender Firewall bunu
varsayılan olarak engeller. İki yol var:

- **Yol A (kolay):** `win-receiver`'ı ilk çalıştırdığında Windows bir **"izin ver"**
  penceresi açar. **"Private networks"** kutusunu işaretleyip **Allow access**'e bas.
- **Yol B (kesin, elle):** Yükseltilmiş (Administrator) bir PowerShell'de kural ekle:

  ```powershell
  netsh advfirewall firewall add rule name="keyboard-it-5599" dir=in action=allow protocol=TCP localport=5599
  ```

  (Test bitince silmek istersen: `netsh advfirewall firewall delete rule name="keyboard-it-5599"`)

---

## 4. Çalıştır

> **Zihinsel model (M0'daki kural hâlâ geçerli):** `SendInput` odaktaki pencereye yazar.
> `win-receiver`'ı çalıştırdığın **terminal öndeyse, "hello" oraya gider, Notepad'e değil.**
> Bu yüzden: win-receiver'ı başlat, sonra **Notepad'i odakla ve öyle tut**; tuşlar Mac'ten
> geldiğinde Notepad ön planda olmalı. (M0'daki 2 saniyelik yarış YOK — istediğin zaman
> gelebilir, yeter ki o an Notepad odakta olsun.)
>
> **Yükseltme eşleşmesi:** Notepad ve win-receiver terminalini **ikisi de normal
> (yükseltilmemiş)** çalıştır.

Adımlar:

1. `keyboard-it` klasörüne gir:
   ```powershell
   cd C:\yol\keyboard-it
   ```

2. **Notepad'i aç** (yükseltilmemiş):
   ```powershell
   notepad
   ```

3. `win-receiver`'ı başlat (ilk sefer derler):
   ```powershell
   cargo run -p win-receiver
   ```
   Şunu yazmalı: `win-receiver dinliyor: 0.0.0.0:5599 — bağlantı bekleniyor`
   (Güvenlik duvarı penceresi çıkarsa **Allow access** — bkz. Bölüm 3.) Program
   **çalışmaya devam eder** (kapatma).

4. **Notepad'i öne getir / tıkla** ki odak orada olsun. Odağı Notepad'te tut.

5. **Kullanıcıya haber ver:** "win-receiver dinliyor, IP = `<bu-pc-ip>`, Notepad odakta —
   Mac'ten `mac-sender`'ı çalıştır." Kullanıcı Mac'te şunu çalıştıracak:
   ```
   cargo run -p mac-sender -- <bu-pc-ip>
   ```

6. Mac gönderdiğinde `win-receiver` terminalinde `bağlandı: ...` satırları belirir ve
   **Notepad'de `hello` yazılır.**

---

## 5. Başarı nasıl doğrulanır

**Başarı ölçütü:** Mac `mac-sender`'ı çalıştırınca Notepad'de `hello` belirir.

Kanıtı makineyle yakala — Notepad metnini geri oku:

```powershell
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty, "Notepad")
$npWin = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
$editCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Document)
$edit = $npWin.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $editCond)
if (-not $edit) {
    $editCond2 = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Edit)
    $edit = $npWin.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $editCond2)
}
$vp = $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
Write-Host "Notepad icerigi: [$($vp.Current.Value)]"
if ($vp.Current.Value.Trim() -eq "hello") { "SONUC: PASS" } else { "SONUC: FAIL" }
```

---

## 6. Sorun giderme

| Belirti | Sebep | Çözüm |
|---|---|---|
| Mac "bağlanılamadı" diyor | Güvenlik duvarı engelliyor / farklı ağ / win-receiver dinlemiyor | Bölüm 3 kuralını ekle; iki makinenin aynı ağda olduğunu doğrula; win-receiver'ın "dinliyor" yazdığını gör. |
| `win-receiver` "dinliyor" yazmıyor, hata veriyor | Port 5599 dolu, ya da bind reddedildi | Başka bir şey 5599 kullanıyor olabilir; win-receiver'ı kapatıp tekrar aç. |
| `bağlandı` göründü ama Notepad boş; "hello" **terminalde** | Enjeksiyon anında odak win-receiver terminalindeydi | Notepad'i öne getir ve odakta tut; Mac'ten tekrar gönder. |
| `bağlandı` var ama hiçbir yere yazılmadı, `SendInput 0/1` uyarısı | Odaktaki pencere yükseltilmiş (UIPI) ya da güvenli masaüstü | Notepad ve terminali normal (yükseltilmemiş) çalıştır; UAC/kilit ekranı olmasın. |
| `eşleme yok: hid=...` uyarısı | O tuş için scancode tablosunda satır yok | M1 testi sadece "hello" gönderiyor; bu çıkmamalı. Çıkarsa bana hid değerini bildir. |
| `okuma/çözme hatası` | Bozuk/yarım çerçeve | Nadir; Mac'ten tekrar gönderin. Tekrarlıyorsa bana tam hata metnini ver. |

---

## 7. Bana ne rapor et

1. **Bu PC'nin IPv4 adresi** (`ipconfig` çıktısındaki).
2. `win-receiver` terminal çıktısı: "dinliyor" satırı + Mac bağlandığında görünen
   `bağlandı: ...` satırları (tam kopya).
3. **Notepad'de "hello" belirdi mi?** `evet`/`hayır` + Bölüm 5'teki `SONUC: PASS/FAIL`.
4. Güvenlik duvarı: izin penceresi mi çıktı yoksa netsh kuralı mı eklendi?
5. Hata varsa: **TAM hata metni** (birebir).
